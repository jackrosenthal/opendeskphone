//! opendeskphone-vpm and opendeskphone-media: the DSP and the real-time audio
//! engine, plus the audio policy that sits between them and the rest of the
//! phone.
//!
//! This module owns the `Audio` topic of the state store and handles the
//! `Audio` command domain (path, mute, volumes, tones, sounds).  Call
//! control reaches the engine through [`AudioService`]'s methods (the SIP
//! wiring adapts them to its `MediaControl` trait).
//!
//! Path policy: an explicit request (speaker or headset key, `SetAudioPath`)
//! wins; otherwise a call or tone uses the headset in headset mode, the
//! handset when it is off the hook, else the speaker.  The ringer always
//! uses the speaker when nothing else has a path open.  With nothing to
//! play and no request, the path is closed.  A request stays until it is
//! withdrawn (`SetAudioPath` with no path, or the handset replaced), so
//! an off-hook dialer keeps its path open between the dial tone and the
//! key tones; a request made for a call is dropped when the last call has
//! gone and nothing is sounding any more.
//!
//! Volume keys: they adjust the volume of the path in use, and the
//! ringer's when the phone is idle or ringing.  When they adjust the
//! ringer's on an idle phone, the ringer sounds so that the user hears
//! the new volume, as on stock: the active ringtone plays once on the
//! ringer's output unless something is already playing, and stops when the
//! volume popup ends, two seconds after the last volume key, or at any
//! other key, the hookswitch, or a call.
//! Further key presses change the volume of the ring that is sounding
//! and do not start it again; so do they for a real ring.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use opendeskphone_hal::{Audio as HalAudioCard, PcmConfig, PcmDevice};
use opendeskphone_media::engine::{Command as Ec, Engine, EngineOptions, Event as Ee, PayloadCodec, SrtpConfig, StreamConfig};
use opendeskphone_media::ring::RingSource;
use opendeskphone_media::tone::ToneSpec;
use opendeskphone_media::{AudioDevice, AudioPath as MPath, DtmfMode, GainControl, VpmDsp, VpmDspOptions};
use opendeskphone_state::{
    Audio, AudioPath, CallId, Command, CommandError, Domain, DspStatus, HealthStatus, Reply, Store, Subsystem, Tone,
    VOLUME_MAX, VolumeTarget,
};
use log::{error, info, warn};

use super::contained;
use super::hal::SharedHal;
use crate::cli::OnOff;
use crate::diag::Output;
use crate::mainrole::{Context, state_model};

/// Stream slots in the engine (three-way conference).
const SLOTS: usize = 3;
/// First RTP port tried; pairs go up from here.
const RTP_PORT_BASE: u16 = 10_000;

/// How long the ringer goes on sounding after the last volume key that
/// adjusted it (stock's volume popup stays this long, and the ringtone it
/// plays ends with it).
const VOLUME_PREVIEW: Duration = Duration::from_secs(2);

/// How long a tone tried in the settings sounds: an alarm tone for this
/// long, a ringtone for at least this long (see [`ringtone_preview`]).
const PREVIEW: Duration = Duration::from_secs(4);

/// What a ringtone tried in the settings plays, once, as `token`: the
/// passes of its ring that begin within [`PREVIEW`], with the ring's pause
/// between them, the last to its end.  A ringtone longer than that is
/// heard whole, and the engine ends the preview where its sound ends.
fn ringtone_preview(source: RingSource, token: u32) -> RingSource {
    let (samples, pause_ms) = match source {
        RingSource::Pcm { samples, pause_ms } => (samples, pause_ms),
        RingSource::Once { samples, start, .. } => return RingSource::Once { samples, start, token },
    };
    let per_ms = (opendeskphone_media::ring::RING_RATE / 1000) as usize;
    let pass = samples.len();
    let period = (pass + pause_ms as usize * per_ms).max(1);
    let passes = (PREVIEW.as_millis() as usize * per_ms).div_ceil(period).max(1);
    let samples = if passes == 1 {
        samples
    } else {
        let mut all = Vec::with_capacity((passes - 1) * period + pass);
        for _ in 1..passes {
            all.extend_from_slice(&samples);
            all.resize(all.len() + period - pass, 0);
        }
        all.extend_from_slice(&samples);
        all.into()
    };
    RingSource::Once { samples, start: 0, token }
}

/// A negotiated stream, in this module's own terms.
#[derive(Clone, Debug)]
pub struct Stream {
    pub codec: PayloadCodec,
    pub tx_pt: u8,
    pub remote: SocketAddr,
    pub ptime_ms: u16,
    pub dtmf_pt: Option<u8>,
    pub srtp: Option<SrtpConfig>,
    pub send: bool,
    pub recv: bool,
}

/// What the engine reports about a call's stream.
#[derive(Clone, Debug)]
pub enum CallMediaEvent {
    Dtmf { call: CallId, digit: char },
    RtpTimeout { call: CallId },
    /// The stream's packet counters, every few seconds.
    Stats { call: CallId, stats: opendeskphone_state::MediaStats },
    Failed { call: CallId, error: String },
}

type Listener = Box<dyn Fn(CallMediaEvent) + Send + Sync>;

/// A sound file an app started.
struct Playback {
    /// Names it in the engine's [`Ee::SoundEnded`].
    token: u32,
    app: String,
}

/// The longest sound file played, in 16 kHz samples (five minutes).
const SOUND_MAX_SAMPLES: usize = 5 * 60 * opendeskphone_media::ring::RING_RATE as usize;
/// The largest sound read.
const SOUND_MAX_BYTES: u64 = 8 << 20;

struct Sockets {
    rtp: UdpSocket,
    rtcp: Option<UdpSocket>,
    port: u16,
}

struct Inner {
    engine: Option<Engine>,
    why_no_engine: String,
    family: opendeskphone_vpm::Family,
    /// Which call uses each engine slot.
    slots: [Option<CallId>; SLOTS],
    /// Sockets reserved by `open` and not yet handed to the engine.
    reserved: HashMap<CallId, Sockets>,
    ports: HashMap<CallId, u16>,
    started: HashMap<CallId, bool>,
    connected: Vec<CallId>,
    /// A multicast page's stream, connected whatever call control says.
    page: Option<CallId>,
    /// Path the user or call control asked for explicitly.
    requested: Option<AudioPath>,
    /// The last call has gone: drop the request once nothing sounds.
    release_when_quiet: bool,
    tone: Option<Tone>,
    ringing: bool,
    /// A ringtone preview or an app's sound file is playing: it gets a
    /// path the way a tone does.
    sound: bool,
    /// The ringtone preview in progress (0: none); tells a preview's
    /// timer whether the preview is still its own.
    preview: u64,
    previews: u64,
    /// The preview in progress is the volume keys' (the ringer sounding
    /// at the volume just set), not a ringtone picked in the settings.
    preview_is_volume: bool,
    /// The engine's token for the sound of the preview in progress when
    /// that is a ringtone tried in the settings ([`ringtone_preview`]),
    /// which ends when the engine reports that the sound has played out.
    preview_sound: Option<u32>,
    /// The ringtone that incoming calls ring with (`active_ringtone`).
    ringtone: String,
    /// The ringer is sounding for the alarm clock.
    alarm: bool,
    /// The sound file playing for an app.
    playback: Option<Playback>,
    /// Sound files stopped or replaced, until the engine says where they
    /// ended.
    ended: Vec<Playback>,
    playbacks: u32,
    /// The engine's current path.
    path: AudioPath,
    next_port: u16,
    bind_ip: IpAddr,
}

/// Handle to the audio subsystem.  Cheap to clone; every method returns
/// quickly (it posts to the audio thread).
#[derive(Clone)]
pub struct AudioService {
    inner: Arc<Mutex<Inner>>,
    store: Store,
    listener: Arc<Mutex<Option<Listener>>>,
}

fn to_media(path: AudioPath) -> Option<MPath> {
    match path {
        AudioPath::None => None,
        AudioPath::Handset => Some(MPath::Handset),
        AudioPath::Headset => Some(MPath::Headset),
        AudioPath::Speaker => Some(MPath::Speaker),
    }
}

/// Receive gain for a volume step: the tuned default at step 5, 3 dB a step.
fn rx_gain_q8(default_q8: i16, level: u8) -> i16 {
    let delta_db = (level.min(VOLUME_MAX) as i32 - 5) * 3;
    (default_q8 as i32 + delta_db * 256).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Ringer gain in Q12 for a volume step: unity at step 5, 3 dB a step, off
/// at 0.  (Table of 10^(dB/20) * 4096, no floating point.)
fn ring_gain_q12(level: u8) -> i32 {
    const TABLE: [i32; 11] = [0, 1029, 1453, 2053, 2900, 4096, 5786, 8173, 11544, 16306, 23033];
    TABLE[level.min(VOLUME_MAX) as usize]
}

fn tone_spec(tone: Tone) -> Option<ToneSpec> {
    Some(match tone {
        Tone::Dial => ToneSpec::DIAL,
        Tone::Ringback => ToneSpec::RINGBACK,
        Tone::Busy => ToneSpec::BUSY,
        Tone::Reorder => ToneSpec::REORDER,
        Tone::CallWaiting => ToneSpec::CALL_WAITING,
        Tone::Confirm => ToneSpec::BEEP,
        Tone::Dtmf(key) => ToneSpec::dtmf(key, -20)?,
    })
}

impl AudioService {
    /// The service with no engine yet.
    fn new(store: Store, family: opendeskphone_vpm::Family) -> AudioService {
        AudioService {
            inner: Arc::new(Mutex::new(Inner {
                engine: None,
                why_no_engine: "not started".into(),
                family,
                slots: [None; SLOTS],
                reserved: HashMap::new(),
                ports: HashMap::new(),
                started: HashMap::new(),
                connected: Vec::new(),
                page: None,
                requested: None,
                release_when_quiet: false,
                tone: None,
                ringing: false,
                sound: false,
                preview: 0,
                previews: 0,
                preview_is_volume: false,
                preview_sound: None,
                ringtone: String::new(),
                alarm: false,
                playback: None,
                ended: Vec::new(),
                playbacks: 0,
                path: AudioPath::None,
                next_port: RTP_PORT_BASE,
                bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            })),
            store,
            listener: Arc::new(Mutex::new(None)),
        }
    }

    /// Codecs that can run now.
    pub fn codecs(&self) -> Vec<PayloadCodec> {
        let inner = self.inner.lock().unwrap();
        if inner.engine.is_none() {
            return Vec::new();
        }
        PayloadCodec::ALL.into_iter().filter(|c| c.is_available(inner.family)).collect()
    }

    /// Whether the audio engine is running at all.
    pub fn available(&self) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        if inner.engine.is_some() { Ok(()) } else { Err(inner.why_no_engine.clone()) }
    }

    /// The engine's level meters, when it runs.
    pub fn meters(&self) -> Option<Arc<opendeskphone_media::Meters>> {
        self.inner.lock().unwrap().engine.as_ref().map(|e| e.meters())
    }

    /// The address RTP sockets bind to follows the phone's address.
    pub fn set_local_ip(&self, ip: IpAddr) {
        self.inner.lock().unwrap().bind_ip = ip;
    }

    pub fn set_listener(&self, listener: Listener) {
        *self.listener.lock().unwrap() = Some(listener);
    }

    /// Reserve an RTP/RTCP socket pair for a call; returns the RTP port.
    pub fn open(&self, call: CallId) -> Result<u16, String> {
        self.end_volume_preview(None);
        let mut inner = self.inner.lock().unwrap();
        if inner.engine.is_none() {
            return Err(format!("audio is not available: {}", inner.why_no_engine));
        }
        inner.release_when_quiet = false;
        if let Some(port) = inner.ports.get(&call) {
            return Ok(*port);
        }
        let start = inner.next_port;
        let pair = opendeskphone_media::net::bind_rtp_pair(IpAddr::V4(Ipv4Addr::UNSPECIFIED), (RTP_PORT_BASE, RTP_PORT_BASE + 2000), start)
            .map_err(|e| format!("no RTP port: {e}"))?;
        // Mark media as the network asks (configuration or the switch's
        // LLDP-MED policy): DSCP, and the socket priority that the voice
        // VLAN maps to its 802.1p value.
        let (dscp, pcp) = self.store.read::<opendeskphone_state::Network, _>(|n| (n.vlan.dscp, n.vlan.voice_priority));
        let marks = opendeskphone_net::QosMarks { rtp_dscp: dscp, rtp_pcp: pcp, ..Default::default() };
        for sock in [&pair.rtp, &pair.rtcp] {
            if let Err(e) = opendeskphone_net::qos::mark_socket(sock, opendeskphone_net::TrafficClass::Rtp, &marks) {
                log::warn!("audio: marking the RTP socket: {e}");
            }
        }
        let port = pair.port();
        inner.next_port = if port + 2 >= RTP_PORT_BASE + 2000 { RTP_PORT_BASE } else { port + 2 };
        inner.reserved.insert(call, Sockets { rtp: pair.rtp, rtcp: Some(pair.rtcp), port });
        inner.ports.insert(call, port);
        Ok(port)
    }

    /// Start or reconfigure a call's stream.
    pub fn start(&self, call: CallId, s: &Stream) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let slot = match inner.slots.iter().position(|c| *c == Some(call)) {
            Some(i) => i,
            None => {
                let free = inner.slots.iter().position(Option::is_none).ok_or("all audio stream slots are in use")?;
                inner.slots[free] = Some(call);
                free
            }
        };
        if let Some(socks) = inner.reserved.remove(&call) {
            let mut cfg = StreamConfig::new(s.codec, s.tx_pt, socks.rtp, s.remote);
            cfg.rtcp = socks.rtcp;
            cfg.remote_rtcp = Some(SocketAddr::new(s.remote.ip(), s.remote.port().wrapping_add(1)));
            cfg.ptime_ms = s.ptime_ms.clamp(10, 60) as u8;
            cfg.dtmf_payload_type = s.dtmf_pt;
            cfg.srtp = s.srtp.clone();
            cfg.send = s.send;
            cfg.recv = s.recv;
            let _ = socks.port;
            inner.send(Ec::StartStream { id: slot, config: Box::new(cfg) })?;
            inner.started.insert(call, true);
        } else if inner.started.contains_key(&call) {
            // Already running: a re-INVITE changed direction or the remote.
            // (A codec or key change needs a fresh socket pair: close, open,
            // start.)
            inner.send(Ec::SetRemote {
                id: slot,
                rtp: s.remote,
                rtcp: Some(SocketAddr::new(s.remote.ip(), s.remote.port().wrapping_add(1))),
            })?;
            inner.send(Ec::SetDirection { id: slot, send: s.send, recv: s.recv })?;
        } else {
            inner.slots[slot] = None;
            return Err("stream started without a reserved socket".into());
        }
        drop(inner);
        self.refresh();
        Ok(())
    }

    /// Stop a call's stream and release its sockets.
    pub fn close(&self, call: CallId) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.reserved.remove(&call);
            inner.ports.remove(&call);
            inner.started.remove(&call);
            inner.connected.retain(|c| *c != call);
            if let Some(slot) = inner.slots.iter().position(|c| *c == Some(call)) {
                inner.slots[slot] = None;
                let _ = inner.send(Ec::StopStream { id: slot });
            }
            if inner.slots.iter().all(Option::is_none) && inner.ports.is_empty() {
                inner.release_when_quiet = true;
            }
        }
        self.refresh();
    }

    pub fn send_dtmf(&self, call: CallId, digit: char, duration_ms: u32) {
        let inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.iter().position(|c| *c == Some(call)) {
            let _ = inner.send(Ec::SendDtmf { id: slot, key: digit, duration_ms, mode: DtmfMode::Auto, feedback: true });
        }
    }

    /// Connect these calls' streams to the audio path (none, one, or a
    /// local conference).
    pub fn connect(&self, calls: &[CallId]) {
        self.inner.lock().unwrap().connected = calls.to_vec();
        self.refresh();
    }

    /// Connect a multicast page's stream to the audio path (`None` to
    /// disconnect).  Unlike [`AudioService::connect`], which call control
    /// replaces wholesale, this stays until changed.
    pub fn connect_page(&self, call: Option<CallId>) {
        if call.is_some() {
            self.end_volume_preview(None);
        }
        self.inner.lock().unwrap().page = call;
        self.refresh();
    }

    /// A Bluetooth headset's voice link came up or went down.  While it is
    /// up the headset path uses the Bluetooth voice device; a headset path
    /// that is open now is reopened on the other device.
    pub fn bluetooth_voice(&self, up: bool) {
        if BLUETOOTH_VOICE.swap(up, std::sync::atomic::Ordering::Relaxed) == up {
            return;
        }
        let inner = self.inner.lock().unwrap();
        if inner.path == AudioPath::Headset {
            let _ = inner.send(Ec::SetPath(None));
            let _ = inner.send(Ec::SetPath(Some(MPath::Headset)));
        }
    }

    pub fn tone(&self, tone: Option<Tone>) {
        if tone.is_some() {
            self.end_volume_preview(None);
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.tone = tone;
            let _ = inner.send(Ec::PlayTone(tone.and_then(tone_spec)));
        }
        self.refresh();
    }

    /// Start or stop the ringer.  `name` picks a built-in ringtone or a
    /// user ringtone from `/nvdata/ringtones`.  The ringer takes the
    /// place of an app's sound file.
    pub fn ring(&self, name: Option<&str>) {
        self.ring_with(name.map(super::ringtones::source));
    }

    /// Start the ringer with this source, or stop it.
    fn ring_with(&self, source: Option<RingSource>) {
        {
            let mut inner = self.inner.lock().unwrap();
            // Whoever rings or silences the ringer now, it is no longer
            // the alarm clock's.
            inner.alarm = false;
            if let Some(p) = inner.playback.take() {
                // The engine reports where the file was cut off.
                inner.ended.push(p);
                inner.sound = false;
            }
            if std::mem::take(&mut inner.preview) != 0 {
                inner.sound = false;
            }
            // The end of a ringtone preview's sound, which the engine
            // reports as this replaces it, ends nothing that follows.
            inner.preview_sound = None;
            inner.ringing = source.is_some();
            let level = self.store.read::<Audio, _>(|a| a.ringer_volume);
            let _ = inner.send(Ec::PlayRing(source, ring_gain_q12(level)));
        }
        self.refresh();
    }

    /// Rings for the alarm clock, with the alarm tone of this name
    /// ([`super::alarmtones`]), until `None`.  The ringer stays everyone else's: a call
    /// that rings, or a sound that plays, takes it over, and `None` then
    /// silences nothing.
    pub fn alarm(&self, tone: Option<&str>) {
        match tone {
            Some(tone) => {
                self.ring_with(Some(super::alarmtones::source(tone)));
                self.inner.lock().unwrap().alarm = true;
            }
            None => {
                let mine = self.inner.lock().unwrap().alarm;
                if mine {
                    self.ring(None);
                }
            }
        }
    }

    /// Plays `samples` (16 kHz) once for `app`, from `start_secs` in, in
    /// place of any other sound file or preview.  It sounds where a tone
    /// would (the path policy), at the path's volume.
    pub fn play_file(&self, samples: Vec<i16>, start_secs: u32, app: String) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.engine.is_none() {
                return Err(format!("no audio: {}", inner.why_no_engine));
            }
            if inner.ringing && inner.preview == 0 {
                return Err("the phone is ringing".into());
            }
            if let Some(p) = inner.playback.take() {
                inner.ended.push(p);
            }
            inner.preview = 0;
            inner.preview_sound = None;
            inner.ringing = false;
            inner.alarm = false;
            inner.playbacks = inner.playbacks.wrapping_add(1);
            let token = inner.playbacks;
            let start = (start_secs as usize).saturating_mul(opendeskphone_media::ring::RING_RATE as usize);
            inner.send(Ec::PlayRing(Some(RingSource::Once { samples: samples.into(), start, token }), 4096))?;
            inner.playback = Some(Playback { token, app });
            inner.sound = true;
        }
        self.refresh();
        Ok(())
    }

    /// Stops a ringtone preview or an app's sound file.
    fn stop_sound(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            let playing = inner.playback.take();
            if playing.is_none() && inner.preview == 0 {
                return;
            }
            inner.preview = 0;
            inner.preview_sound = None;
            inner.sound = false;
            inner.ringing = false;
            let sent = inner.send(Ec::PlayRing(None, 4096));
            match (playing, sent) {
                (Some(p), Ok(())) => inner.ended.push(p),
                (Some(p), Err(_)) => {
                    self.store.emit(opendeskphone_state::Event::PlaybackStopped { app: p.app, position_secs: 0 });
                }
                (None, _) => {}
            }
        }
        self.refresh();
    }

    /// The engine's report that a sound file (or a ringtone preview's
    /// sound) ended at sample `position`.
    fn sound_ended(&self, token: u32, position: usize) {
        let (app, current) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.playback.as_ref().is_some_and(|p| p.token == token) {
                // It played to its end.
                inner.sound = false;
                (inner.playback.take().map(|p| p.app), true)
            } else if inner.preview_sound == Some(token) {
                // The ringtone tried in the settings has played out.
                inner.preview = 0;
                inner.preview_sound = None;
                inner.sound = false;
                inner.ringing = false;
                (None, true)
            } else {
                let i = inner.ended.iter().position(|p| p.token == token);
                (i.map(|i| inner.ended.remove(i).app), false)
            }
        };
        if current {
            self.refresh();
        }
        if let Some(app) = app {
            let position_secs = (position / opendeskphone_media::ring::RING_RATE as usize) as u32;
            self.store.emit(opendeskphone_state::Event::PlaybackStopped { app, position_secs });
        }
    }

    pub fn request_path(&self, path: AudioPath) {
        if path != AudioPath::None {
            self.end_volume_preview(None);
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.requested = (path != AudioPath::None).then_some(path);
            inner.release_when_quiet = false;
        }
        self.refresh();
    }

    /// Take the volume settings and the ringtone from the merged
    /// configuration.
    pub fn apply_config(&self, cfg: &opendeskphone_config::Config) {
        let level = |id: &str, now: u8| cfg.setting_num::<u8>(id).map(|v| v.min(VOLUME_MAX)).unwrap_or(now);
        self.inner.lock().unwrap().ringtone = cfg.ringtones().active;
        self.store.update::<Audio, _>(|a| {
            a.handset_volume = level("handset_volume", a.handset_volume);
            a.headset_volume = level("headset_volume", a.headset_volume);
            a.speaker_volume = level("speaker_volume", a.speaker_volume);
            a.ringer_volume = level("ringer_volume", a.ringer_volume);
        });
    }

    /// Recompute the path and the conference mask and publish the state.
    fn refresh(&self) {
        let (off_hook, headset_mode, volumes) =
            self.store.read::<Audio, _>(|a| (a.off_hook, a.headset_mode, (a.handset_volume, a.headset_volume, a.speaker_volume)));
        let mut inner = self.inner.lock().unwrap();
        let wants_voice = !inner.connected.is_empty() || inner.page.is_some() || inner.tone.is_some() || inner.sound;
        if inner.release_when_quiet && !wants_voice && !inner.ringing && inner.slots.iter().all(Option::is_none) {
            // The calls are gone and their last tone has ended: a request
            // made for them does not outlive that.
            inner.requested = None;
            inner.release_when_quiet = false;
        }
        let policy = if headset_mode {
            AudioPath::Headset
        } else if off_hook {
            AudioPath::Handset
        } else {
            AudioPath::Speaker
        };
        let path = match (inner.requested, wants_voice, inner.ringing) {
            (Some(p), _, _) => p,
            (None, true, _) => policy,
            (None, false, true) => AudioPath::Speaker,
            (None, false, false) => AudioPath::None,
        };
        if path != inner.path {
            if let Some(mp) = to_media(path) {
                let (default, level) = match path {
                    AudioPath::Handset => (inner.default_gain(MPath::Handset), volumes.0),
                    AudioPath::Headset => (inner.default_gain(MPath::Headset), volumes.1),
                    _ => (inner.default_gain(MPath::Speaker), volumes.2),
                };
                let _ = inner.send(Ec::SetRxGain { path: mp, gain_q8: rx_gain_q8(default, level) });
            }
            let _ = inner.send(Ec::SetPath(to_media(path)));
            inner.path = path;
        }
        let mut mask = 0u8;
        for (slot, call) in inner.slots.iter().enumerate() {
            if call.is_some_and(|c| inner.connected.contains(&c) || inner.page == Some(c)) {
                mask |= 1 << slot;
            }
        }
        let _ = inner.send(Ec::SetConference { mask });
        let (tone, ringing) = (inner.tone, inner.ringing);
        drop(inner);
        self.store.update::<Audio, _>(|a| {
            a.path = path;
            a.tone = tone;
            a.ringing = ringing;
        });
    }

    /// Sets a volume; returns which one `target` was (never `Current`).
    fn set_volume(&self, target: VolumeTarget, level: u8) -> VolumeTarget {
        let level = level.min(VOLUME_MAX);
        let (path, ringing) = {
            let inner = self.inner.lock().unwrap();
            (inner.path, inner.ringing)
        };
        let target = match target {
            VolumeTarget::Current => match path {
                AudioPath::Handset => VolumeTarget::Handset,
                AudioPath::Headset => VolumeTarget::Headset,
                AudioPath::Speaker if !ringing => VolumeTarget::Speaker,
                _ => VolumeTarget::Ringer,
            },
            t => t,
        };
        self.store.update::<Audio, _>(|a| match target {
            VolumeTarget::Handset => a.handset_volume = level,
            VolumeTarget::Headset => a.headset_volume = level,
            VolumeTarget::Speaker => a.speaker_volume = level,
            VolumeTarget::Ringer | VolumeTarget::Current => a.ringer_volume = level,
        });
        let inner = self.inner.lock().unwrap();
        let apply = |mp: MPath| {
            let _ = inner.send(Ec::SetRxGain { path: mp, gain_q8: rx_gain_q8(inner.default_gain(mp), level) });
        };
        match target {
            VolumeTarget::Handset => apply(MPath::Handset),
            VolumeTarget::Headset => apply(MPath::Headset),
            VolumeTarget::Speaker => apply(MPath::Speaker),
            VolumeTarget::Ringer | VolumeTarget::Current => {
                // A ring that is sounding (for a call, or a preview) goes
                // on at the new volume; an app's sound file, which plays
                // in the ring's place, is not the ringer's to scale.
                if inner.ringing && inner.playback.is_none() {
                    let _ = inner.send(Ec::SetRingGain(ring_gain_q12(level)));
                }
            }
        }
        target
    }

    /// The volume keys adjusted the ringer's volume: on an idle phone the
    /// ringer sounds, with the ringtone calls ring with, until
    /// [`VOLUME_PREVIEW`] after the last such key (or until something
    /// else happens: see [`AudioService::end_volume_preview`]).  A ring
    /// that is already sounding is left to go on, at the new volume.
    fn volume_preview(&self) {
        let (calls, off_hook) =
            (self.store.read::<opendeskphone_state::Calls, _>(|c| !c.is_idle()), self.store.read::<Audio, _>(|a| a.off_hook));
        let (mine, started) = {
            let mut inner = self.inner.lock().unwrap();
            let started = inner.preview == 0;
            if started {
                let busy = calls
                    || off_hook
                    || inner.ringing
                    || inner.path != AudioPath::None
                    || inner.requested.is_some()
                    || inner.tone.is_some()
                    || inner.sound
                    || inner.playback.is_some()
                    || inner.page.is_some()
                    || !inner.connected.is_empty()
                    || !inner.ports.is_empty()
                    || inner.slots.iter().any(Option::is_some);
                if busy {
                    return;
                }
                inner.ringing = true;
                let source = super::ringtones::source(&inner.ringtone);
                let level = self.store.read::<Audio, _>(|a| a.ringer_volume);
                let _ = inner.send(Ec::PlayRing(Some(source), ring_gain_q12(level)));
            } else if !inner.preview_is_volume {
                // A ringtone picked in the settings is sounding; it ends
                // by its own clock.
                return;
            }
            inner.previews += 1;
            inner.preview = inner.previews;
            inner.preview_is_volume = true;
            (inner.preview, started)
        };
        if started {
            self.refresh();
        }
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(VOLUME_PREVIEW).await;
            me.end_volume_preview(Some(mine));
        });
    }

    /// Stops the ringer if it is sounding for the volume keys: `Some` is
    /// the preview whose time is up (nothing if a later key press has
    /// taken it over), `None` anything that ends it at once (another key,
    /// the hookswitch, a call, a tone, a path request).  A real ring and
    /// a ringtone picked in the settings are not touched.
    fn end_volume_preview(&self, timed_out: Option<u64>) {
        {
            let inner = self.inner.lock().unwrap();
            if inner.preview == 0 || !inner.preview_is_volume || timed_out.is_some_and(|t| t != inner.preview) {
                return;
            }
        }
        self.stop_sound();
    }

    /// A key or the hookswitch, from the event stream.
    fn input_event(&self, event: &opendeskphone_state::Event) {
        match *event {
            opendeskphone_state::Event::Hook { off_hook, .. } => {
                self.end_volume_preview(None);
                self.store.update::<Audio, _>(|a| a.off_hook = off_hook);
                {
                    let mut inner = self.inner.lock().unwrap();
                    // Lifting the handset takes the audio there; replacing it
                    // drops an explicit handset choice (call control decides
                    // whether the call ends).
                    if off_hook {
                        if inner.path != AudioPath::None && !inner.ringing {
                            inner.requested = Some(AudioPath::Handset);
                        }
                    } else if inner.requested == Some(AudioPath::Handset) {
                        inner.requested = None;
                    }
                }
                self.refresh();
            }
            opendeskphone_state::Event::Key { key, down: true, .. }
                if !matches!(key, opendeskphone_state::Key::VolumeUp | opendeskphone_state::Key::VolumeDown) =>
            {
                self.end_volume_preview(None);
            }
            _ => {}
        }
    }

    fn current_level(&self, target: VolumeTarget) -> u8 {
        let path = self.inner.lock().unwrap().path;
        self.store.read::<Audio, _>(|a| match target {
            VolumeTarget::Handset => a.handset_volume,
            VolumeTarget::Headset => a.headset_volume,
            VolumeTarget::Speaker => a.speaker_volume,
            VolumeTarget::Ringer => a.ringer_volume,
            VolumeTarget::Current => match path {
                AudioPath::Handset => a.handset_volume,
                AudioPath::Headset => a.headset_volume,
                AudioPath::Speaker if !a.ringing => a.speaker_volume,
                _ => a.ringer_volume,
            },
        })
    }

    /// One command of the `Audio` domain.
    fn command(&self, cmd: Command) -> Result<Reply, CommandError> {
        match cmd {
            Command::SetAudioPath { path } => self.request_path(path),
            Command::SetMute { on } => {
                let _ = self.inner.lock().unwrap().send(Ec::MuteMic(on));
                self.store.update::<Audio, _>(|a| a.muted = on);
            }
            Command::SetVolume { target, level } => {
                self.set_volume(target, level);
            }
            Command::AdjustVolume { target, delta } => {
                let now = self.current_level(target) as i16;
                let which = self.set_volume(target, (now + delta as i16).clamp(0, VOLUME_MAX as i16) as u8);
                // The volume keys (they step the volume in use), on the
                // ringer's: let the user hear it, even at a limit.
                if target == VolumeTarget::Current && which == VolumeTarget::Ringer {
                    self.volume_preview();
                }
            }
            Command::SetHeadsetMode { on } => {
                self.store.update::<Audio, _>(|a| a.headset_mode = on);
                self.refresh();
            }
            Command::PlayTone { tone, duration_ms } => {
                self.tone(Some(tone));
                if duration_ms > 0 {
                    let me = self.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(duration_ms as u64)).await;
                        if me.inner.lock().unwrap().tone == Some(tone) {
                            me.tone(None);
                        }
                    });
                }
            }
            Command::StopTone => self.tone(None),
            Command::PlaySound { name } => {
                // A preview of the named ringtone: no time cuts it off;
                // the engine ends it with its sound.
                let token = {
                    let mut inner = self.inner.lock().unwrap();
                    inner.playbacks = inner.playbacks.wrapping_add(1);
                    inner.playbacks
                };
                self.ring_with(Some(ringtone_preview(super::ringtones::source(&name), token)));
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.sound = true;
                    inner.previews += 1;
                    inner.preview = inner.previews;
                    inner.preview_is_volume = false;
                    inner.preview_sound = Some(token);
                }
                self.refresh();
            }
            Command::PlayAlarmTone { name } => {
                // A preview of the named alarm tone, for a few seconds.
                self.ring_with(Some(super::alarmtones::source(&name)));
                let mine = {
                    let mut inner = self.inner.lock().unwrap();
                    inner.sound = true;
                    inner.previews += 1;
                    inner.preview = inner.previews;
                    inner.preview_is_volume = false;
                    inner.preview
                };
                self.refresh();
                let me = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(PREVIEW).await;
                    if me.inner.lock().unwrap().preview == mine {
                        me.stop_sound();
                    }
                });
            }
            Command::StopSound => self.stop_sound(),
            other => return Err(CommandError::Invalid(format!("not an audio command: {other:?}"))),
        }
        Ok(Reply::Done)
    }
}

impl Inner {
    fn send(&self, cmd: Ec) -> Result<(), String> {
        match &self.engine {
            Some(e) => e.send(cmd).map_err(|e| e.to_string()),
            None => Err(format!("audio is not available: {}", self.why_no_engine)),
        }
    }

    fn default_gain(&self, path: MPath) -> i16 {
        let g = opendeskphone_vpm::startup::tuning(self.family).gains;
        match path {
            MPath::Handset => g.handset,
            MPath::Headset => g.headset,
            MPath::Speaker => g.speaker,
        }
    }
}

// ------------------------------------------------------------------------
// The sound card behind the engine's device trait.

struct CardDevice {
    card: Box<dyn HalAudioCard>,
    model: opendeskphone_state::Model,
    pcm: Option<Box<dyn opendeskphone_hal::Pcm>>,
    open: Option<MPath>,
    /// The open device is the Bluetooth headset's, at 8 kHz: this many
    /// engine samples per Bluetooth sample.
    bluetooth: Option<usize>,
    /// Conversion buffer, and the last sample read (for interpolation).
    narrow: Vec<i16>,
    last: i16,
}

/// A Bluetooth headset's voice link is up: the headset path is then the
/// Bluetooth voice device.  Set by [`AudioService::bluetooth_voice`].
static BLUETOOTH_VOICE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 8 kHz samples to the engine's rate (`factor` times as many), by linear
/// interpolation from the previous sample.
fn widen(narrow: &[i16], last: &mut i16, factor: usize, out: &mut [i16]) {
    for (i, &sample) in narrow.iter().enumerate() {
        let (a, b) = (*last as i32, sample as i32);
        for k in 0..factor {
            if let Some(o) = out.get_mut(i * factor + k) {
                *o = (a + (b - a) * (k as i32 + 1) / factor as i32) as i16;
            }
        }
        *last = sample;
    }
}

/// The engine's rate to 8 kHz: the mean of each `factor` samples.
fn narrow(wide: &[i16], factor: usize, out: &mut Vec<i16>) {
    out.clear();
    out.extend(wide.chunks_exact(factor).map(|c| (c.iter().map(|s| *s as i32).sum::<i32>() / factor as i32) as i16));
}

fn pcm_device(path: MPath) -> PcmDevice {
    match path {
        MPath::Handset => PcmDevice::Handset,
        MPath::Headset => PcmDevice::Headset,
        MPath::Speaker => PcmDevice::Speaker,
    }
}

fn io_err(e: opendeskphone_hal::HalError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

impl AudioDevice for CardDevice {
    fn rate(&self) -> u32 {
        self.card.rate()
    }

    fn open(&mut self, path: MPath) -> std::io::Result<()> {
        self.close();
        if path == MPath::Headset && BLUETOOTH_VOICE.load(std::sync::atomic::Ordering::Relaxed) {
            let factor = (self.card.rate() / opendeskphone_hal::BLUETOOTH_RATE).max(1) as usize;
            let config = PcmConfig { period_frames: opendeskphone_hal::BLUETOOTH_RATE / 200, periods: 4, capture: true, playback: true };
            match self.card.open_bluetooth(config) {
                Ok(pcm) => {
                    self.pcm = Some(pcm);
                    self.open = Some(path);
                    self.bluetooth = Some(factor);
                    return Ok(());
                }
                // Fall back to the wired headset jack.
                Err(e) => warn!("audio: Bluetooth voice device: {e}"),
            }
        }
        let device = pcm_device(path);
        // The mainline driver does not apply the board's gain table.
        match self.card.mixer() {
            Ok(mut mixer) => {
                if let Err(e) = opendeskphone_hal::apply_board_gains(mixer.as_mut(), self.model, device, 0) {
                    warn!("audio: board gains for {device:?}: {e}");
                }
            }
            Err(e) => warn!("audio: mixer: {e}"),
        }
        let pcm = self.card.open(device, PcmConfig::voice(self.model)).map_err(io_err)?;
        self.pcm = Some(pcm);
        self.open = Some(path);
        Ok(())
    }

    fn close(&mut self) {
        self.pcm = None;
        self.open = None;
        self.bluetooth = None;
        self.last = 0;
    }

    fn read(&mut self, buf: &mut [i16]) -> std::io::Result<()> {
        let pcm = self.pcm.as_mut().ok_or_else(|| std::io::Error::other("no PCM open"))?;
        if let Some(factor) = self.bluetooth {
            self.narrow.resize(buf.len() / factor, 0);
            return match pcm.read(&mut self.narrow) {
                Ok(()) => {
                    widen(&self.narrow, &mut self.last, factor, buf);
                    Ok(())
                }
                Err(opendeskphone_hal::HalError::Xrun) => {
                    pcm.recover().map_err(io_err)?;
                    buf.fill(0);
                    Ok(())
                }
                Err(e) => Err(io_err(e)),
            };
        }
        match pcm.read(buf) {
            Ok(()) => Ok(()),
            Err(opendeskphone_hal::HalError::Xrun) => {
                pcm.recover().map_err(io_err)?;
                buf.fill(0);
                Ok(())
            }
            Err(e) => Err(io_err(e)),
        }
    }

    fn write(&mut self, buf: &[i16]) -> std::io::Result<()> {
        let pcm = self.pcm.as_mut().ok_or_else(|| std::io::Error::other("no PCM open"))?;
        if let Some(factor) = self.bluetooth {
            narrow(buf, factor, &mut self.narrow);
            return match pcm.write(&self.narrow) {
                Ok(()) => Ok(()),
                Err(opendeskphone_hal::HalError::Xrun) => pcm.recover().map_err(io_err),
                Err(e) => Err(io_err(e)),
            };
        }
        match pcm.write(buf) {
            Ok(()) => Ok(()),
            Err(opendeskphone_hal::HalError::Xrun) => pcm.recover().map_err(io_err),
            Err(e) => Err(io_err(e)),
        }
    }

    fn set_gain(&mut self, _path: MPath, control: GainControl, value: i32) -> std::io::Result<()> {
        // Front-end gains come from the board table at open; the user's
        // volume is the DSP's receive gain.  Explicit overrides are for
        // diagnostics only.
        warn!("audio: device gain {control:?}={value} ignored (board table in force)");
        Ok(())
    }
}

// ------------------------------------------------------------------------
// Start-up.

type Built = (Box<dyn AudioDevice>, Box<dyn opendeskphone_media::Dsp>);

/// Start the DSP and build the engine's two devices.  Runs on its own
/// thread: starting the remoteproc loads firmware and can take seconds.
fn build_phone(card: Box<dyn HalAudioCard>, model: opendeskphone_state::Model) -> Result<Built, String> {
    let rproc = opendeskphone_vpm::remoteproc::Remoteproc::find().map_err(|e| format!("DSP remoteproc: {e}"))?;
    // A DSP left running by an earlier main role still holds that run's
    // canceller instances, and a second Init of one never answers (seen on
    // a D60: the start-up stopped at the handset canceller's Init).  Start
    // from a freshly loaded DSP every time.
    rproc.restart().map_err(|e| format!("DSP start: {e}"))?;
    let device = opendeskphone_vpm::Device::open_default().map_err(|e| format!("/dev/bcmring-vpm: {e}"))?;
    let vpm = opendeskphone_vpm::Vpm::new(device).map_err(|e| format!("DSP info: {e}"))?;
    let dsp = VpmDsp::new(vpm, VpmDspOptions::default());
    Ok((Box::new(CardDevice { card, model, pcm: None, open: None, bluetooth: None, narrow: Vec::new(), last: 0 }), Box::new(dsp)))
}

fn build_sim(family: opendeskphone_vpm::Family) -> Result<Built, String> {
    let (fake, state) = opendeskphone_media::fake::FakeAudio::new(family.device_rate());
    {
        let mut s = state.lock().unwrap();
        s.record = false;
        s.read_delay = Some(Duration::from_millis(10));
    }
    let vpm = opendeskphone_vpm::Vpm::new(opendeskphone_vpm::MockTransport::new(family)).map_err(|e| e.to_string())?;
    Ok((Box::new(fake), Box::new(VpmDsp::new(vpm, VpmDspOptions::default()))))
}

pub async fn start(ctx: &Context, hal: Option<SharedHal>) -> AudioService {
    ctx.store.set_health(Subsystem::Media, HealthStatus::Starting);
    let model = state_model(ctx.model);
    let family = match ctx.model.family() {
        crate::model::Family::Old => opendeskphone_vpm::Family::Old,
        crate::model::Family::New => opendeskphone_vpm::Family::New,
        // The D80 has no such voice processor; its audio is still to be
        // done.  The simulator gives it the new family's.
        crate::model::Family::Touch => opendeskphone_vpm::Family::New,
    };
    let service = AudioService::new(ctx.store.clone(), family);
    ctx.store.update::<Audio, _>(|a| a.dsp = DspStatus::Starting);

    let built = if ctx.sim {
        build_sim(family)
    } else {
        // Take the sound card out of the hardware bundle; the engine owns it.
        let card = hal.and_then(|h| {
            let mut h = h.lock().ok()?;
            if h.missing.iter().any(|(name, _)| *name == "audio") {
                return None;
            }
            Some(std::mem::replace(&mut h.audio, Box::new(opendeskphone_hal::Null("audio"))))
        });
        match card {
            Some(card) => contained("dsp-start", Duration::from_secs(30), move || build_phone(card, model))
                .await
                .and_then(|r| r),
            None => Err("no sound card".to_string()),
        }
    };

    let (ev_tx, ev_rx) = tokio::sync::mpsc::unbounded_channel::<Ee>();
    match built.and_then(|(audio, dsp)| {
        Engine::spawn(
            audio,
            dsp,
            move |e| {
                let _ = ev_tx.send(e);
            },
            EngineOptions::default(),
        )
        .map_err(|e| format!("audio thread: {e}"))
    }) {
        Ok(engine) => {
            let mut inner = service.inner.lock().unwrap();
            inner.engine = Some(engine);
            inner.why_no_engine.clear();
            drop(inner);
            info!("audio: engine running ({family:?} family{})", if ctx.sim { ", simulated devices" } else { "" });
            ctx.store.update::<Audio, _>(|a| a.dsp = DspStatus::Running);
            ctx.store.set_health(Subsystem::Media, HealthStatus::Ok);
        }
        Err(e) => {
            error!("audio: {e}; the phone runs without audio");
            service.inner.lock().unwrap().why_no_engine = e.clone();
            ctx.store.update::<Audio, _>(|a| a.dsp = DspStatus::Failed(e.clone()));
            ctx.store.set_health(Subsystem::Media, HealthStatus::Failed(e));
        }
    }

    tokio::spawn(events(service.clone(), ev_rx, ctx.sim));
    tokio::spawn(commands(service.clone(), ctx.store.clone()));
    tokio::spawn(hook_watch(service.clone(), ctx.store.clone()));
    tokio::spawn(lamp_watch(service.clone(), ctx.store.clone()));
    register_diag(ctx, service.clone());
    service
}

/// Engine events, on the runtime.
async fn events(service: AudioService, mut rx: tokio::sync::mpsc::UnboundedReceiver<Ee>, sim: bool) {
    let mut restarts = 0u32;
    while let Some(event) = rx.recv().await {
        let call_of = |id: usize| service.inner.lock().unwrap().slots.get(id).copied().flatten();
        let tell = |e: CallMediaEvent| {
            if let Some(l) = service.listener.lock().unwrap().as_ref() {
                l(e);
            }
        };
        match event {
            Ee::Dtmf { id, edge } => {
                if let (Some(call), opendeskphone_media::dtmf::DtmfEdge::Start(digit)) = (call_of(id), edge) {
                    tell(CallMediaEvent::Dtmf { call, digit });
                }
            }
            Ee::RtpTimeout { id } => {
                if let Some(call) = call_of(id) {
                    tell(CallMediaEvent::RtpTimeout { call });
                }
            }
            Ee::Stats { id, stats } => {
                if let Some(call) = call_of(id) {
                    let stats = opendeskphone_state::MediaStats {
                        rx_packets: stats.packets_received,
                        tx_packets: stats.packets_sent,
                        rx_lost: stats.packets_lost.max(0) as u32,
                        jitter_ms: stats.jitter_ms.min(u32::from(u16::MAX)) as u16,
                    };
                    tell(CallMediaEvent::Stats { call, stats });
                }
            }
            Ee::StreamError { id, error } => {
                warn!("audio: stream {id}: {error}");
                if let Some(call) = call_of(id) {
                    tell(CallMediaEvent::Failed { call, error });
                }
            }
            Ee::DspError { error, needs_restart } => {
                error!("audio: DSP: {error}{}", if needs_restart { " (restart needed)" } else { "" });
                if needs_restart && !sim {
                    restarts += 1;
                    if restarts > 5 {
                        service.store.update::<Audio, _>(|a| a.dsp = DspStatus::Failed(format!("{error}; gave up after 5 restarts")));
                        service.store.set_health(Subsystem::Media, HealthStatus::Failed(error.to_string()));
                        continue;
                    }
                    service.store.update::<Audio, _>(|a| a.dsp = DspStatus::Starting);
                    let result = contained("dsp-restart", Duration::from_secs(30), || {
                        opendeskphone_vpm::remoteproc::Remoteproc::find().and_then(|r| r.restart()).map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|r| r);
                    match result {
                        Ok(()) => {
                            let _ = service.inner.lock().unwrap().send(Ec::RestartDsp);
                            service.store.update::<Audio, _>(|a| a.dsp = DspStatus::Running);
                            info!("audio: DSP restarted");
                        }
                        Err(e) => {
                            error!("audio: DSP restart: {e}");
                            service.store.update::<Audio, _>(|a| a.dsp = DspStatus::Failed(e.clone()));
                            service.store.set_health(Subsystem::Media, HealthStatus::Failed(e));
                        }
                    }
                }
            }
            Ee::AudioError { kind } => warn!("audio: sound card: {kind:?}"),
            Ee::RealtimeRefused { priority, errno } => warn!(
                "audio: SCHED_FIFO priority {priority} was refused ({}); the audio thread runs at ordinary priority",
                std::io::Error::from_raw_os_error(errno)
            ),
            Ee::DspStartup { summary } => info!("audio: DSP: {summary}"),
            Ee::ToneEnded => {
                service.inner.lock().unwrap().tone = None;
                service.refresh();
            }
            Ee::SoundEnded { token, position } => service.sound_ended(token, position),
        }
    }
}

/// The `Audio` command domain.
async fn commands(service: AudioService, store: Store) {
    let rx = store.handle(Domain::Audio);
    while let Some(envelope) = rx.next().await {
        let (cmd, responder) = envelope.split();
        match cmd {
            Command::PlayData { sound, start_secs, app } => {
                // Decoding what an app hands over is not for this thread.
                let service = service.clone();
                tokio::spawn(async move {
                    let loaded = tokio::task::spawn_blocking(move || decode_checked(&sound.0))
                        .await
                        .unwrap_or_else(|e| Err(format!("decoding the sound failed: {e}")));
                    let result = loaded.and_then(|samples| service.play_file(samples, start_secs, app));
                    responder.reply(result.map(|()| Reply::Done).map_err(CommandError::Failed));
                });
            }
            cmd => responder.reply(service.command(cmd)),
        }
    }
}

/// The content of a sound file as 16 kHz mono samples.
fn decode_checked(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() as u64 > SOUND_MAX_BYTES {
        return Err("the sound is larger than 8 MiB".into());
    }
    let samples = decode_sound(bytes)?;
    if samples.len() > SOUND_MAX_SAMPLES {
        return Err("the sound is longer than five minutes".into());
    }
    Ok(samples)
}

/// Decodes a sound file.  Stock's format is headerless 16 kHz mono
/// signed 16-bit little-endian samples (`.sln16`; its ringtones and what
/// its `playFile` reads); anything that does not start as a RIFF file is
/// taken as that.  RIFF/WAVE files (which stock converts to `.sln16` on
/// upload) are decoded here: 8-bit or 16-bit PCM, mu-law, or A-law at any
/// rate, the first channel of several.
fn decode_sound(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.starts_with(b"RIFF") {
        return opendeskphone_media::ring::decode_wav(bytes).map_err(|e| e.to_string());
    }
    Ok(bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect())
}

/// Keep `Audio::off_hook` current, move a call between handset and
/// speaker when the handset is lifted or replaced, and end the volume
/// keys' ringer preview at any other key.
async fn hook_watch(service: AudioService, store: Store) {
    let sub = store.subscribe_events(opendeskphone_state::Topic::Audio);
    loop {
        let wake = sub.changed().await;
        for event in wake.events {
            service.input_event(&event);
        }
    }
}

/// Keep the speaker, headset, and mute lamps current, and take the mute
/// off where stock does: when a call appears, and when a call is put on
/// hold or taken back (stock's `phone` sends `audio_mute("off")` at call
/// creation, hold, and resume).
async fn lamp_watch(service: AudioService, store: Store) {
    use opendeskphone_state::{Calls, Keys, Topic};
    let topics: opendeskphone_state::TopicSet = [Topic::Audio, Topic::Calls].into_iter().collect();
    let sub = store.subscribe(topics);
    // Each call with whether it is held, as last seen.
    let mut seen: Vec<(CallId, bool)> = Vec::new();
    loop {
        let calls = store.get::<Calls>();
        let now: Vec<(CallId, bool)> = calls.live().map(|c| (c.id, c.state.is_held())).collect();
        let changed = now.iter().any(|(id, held)| seen.iter().find(|(s, _)| s == id).is_none_or(|(_, h)| h != held));
        seen = now;
        if changed && store.read::<Audio, _>(|a| a.muted) {
            let _ = service.command(Command::SetMute { on: false });
        }
        let lamps = opendeskphone_state::audio_lamps(&store.get::<Audio>(), &calls);
        store.update::<Keys, _>(|k| (k.speaker_lamp, k.headset_lamp, k.mute_lamp) = lamps);
        sub.changed().await;
    }
}

/// The audio engine: status, and paths, tones, rings, and volumes by hand
///
/// Without a subcommand: status.
#[derive(Debug, Parser)]
#[command(name = "audio")]
pub struct AudioCommand {
    #[command(subcommand)]
    action: Option<AudioAction>,
}

#[derive(Debug, Subcommand)]
enum AudioAction {
    /// Show the engine, the codecs, the levels, and the audio state
    Status,
    /// Open an audio path (none closes it)
    Path { path: PathArg },
    /// Play a call-progress tone (off stops it)
    Tone { tone: ToneArg },
    /// Ring
    Ring {
        /// The ring to play (the default ring if not given), or off
        name: Option<String>,
    },
    /// Set a volume
    Volume {
        target: VolumeArg,
        #[arg(value_parser = clap::value_parser!(u8).range(0..=10))]
        level: u8,
    },
    /// Mute or unmute the microphone
    Mute { state: OnOff },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PathArg {
    None,
    Handset,
    Headset,
    Speaker,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ToneArg {
    Dial,
    Ringback,
    Busy,
    Beep,
    Off,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VolumeArg {
    Handset,
    Headset,
    Speaker,
    Ringer,
    /// Whatever sounds now
    Current,
}

/// The DSP: status, self-test, restart, and experiments
///
/// Without a subcommand: status.
#[derive(Debug, Parser)]
#[command(name = "dsp")]
pub struct DspCommand {
    #[command(subcommand)]
    action: Option<DspAction>,
}

#[derive(Debug, Subcommand)]
enum DspAction {
    /// Show the engine, the remote processor, and the driver's counters
    Status,
    /// Run the checks of the DSP protocol code that need no hardware
    Selftest,
    /// Reload the DSP's firmware and start it again
    Restart,
    /// Send one job to the DSP and print its result
    Job {
        client: u16,
        channel: u16,
        instance: u16,
        op: u16,
        arg1: u16,
        arg2: u16,
        /// The length of the job's body
        body_bytes: u16,
        /// The 16-bit word the body is filled with
        #[arg(default_value_t = 0)]
        fill_word: u16,
    },
    /// Put a test signal through the filter bank of a stream
    #[command(group = clap::ArgGroup::new("signal").required(true))]
    Bands {
        /// The stream: 0 to 3
        #[arg(value_parser = clap::value_parser!(u8).range(0..4))]
        stream: u8,
        /// A tone of this frequency
        #[arg(long, value_name = "HZ", group = "signal", value_parser = clap::value_parser!(u32).range(1..16000))]
        tone: Option<u32>,
        /// A sweep
        #[arg(long, group = "signal")]
        sweep: bool,
        #[command(flatten)]
        frames: Frames,
    },
    /// Put a tone in one band buffer through one canceller
    Canceller {
        canceller: CancellerArg,
        direction: DirectionArg,
        /// The band buffer: 0, 1, or 2
        buffer: usize,
        /// The tone's frequency
        hz: u32,
        #[command(flatten)]
        frames: Frames,
    },
}

#[derive(Clone, Copy, Debug, clap::Args)]
struct Frames {
    /// How many frames to run
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(5..=1000))]
    frames: u16,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CancellerArg {
    Hec0,
    Hec1,
    Sbaec,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DirectionArg {
    Ingress,
    Egress,
}

fn register_diag(ctx: &Context, service: AudioService) {
    let store = ctx.store.clone();
    let svc = service.clone();
    ctx.diag.register(move |args: AudioCommand, out: Output| {
        let (svc, store) = (svc.clone(), store.clone());
        async move {
            let result: Result<String, String> = match args.action.unwrap_or(AudioAction::Status) {
                AudioAction::Status => {
                    let a = store.get::<Audio>();
                    let engine = match svc.available() {
                        Ok(()) => "running".to_string(),
                        Err(e) => format!("NOT running: {e}"),
                    };
                    let codecs: Vec<String> = svc.codecs().iter().map(|c| format!("{c:?}")).collect();
                    let levels = match svc.meters() {
                        Some(m) => {
                            use std::sync::atomic::Ordering::Relaxed;
                            format!(
                                "levels (last second, of 32767): to device peak {} mean {}; from device peak {} mean {}\nticks: {}  DSP job failures: {}\n",
                                m.speaker_peak.load(Relaxed),
                                m.speaker_mean.load(Relaxed),
                                m.mic_peak.load(Relaxed),
                                m.mic_mean.load(Relaxed),
                                m.ticks.load(Relaxed),
                                m.dsp_errors.load(Relaxed),
                            )
                        }
                        None => String::new(),
                    };
                    Ok(format!("engine: {engine}\ncodecs: {}\n{levels}{a:#?}\n", codecs.join(" ")))
                }
                AudioAction::Path { path } => {
                    svc.request_path(match path {
                        PathArg::None => AudioPath::None,
                        PathArg::Handset => AudioPath::Handset,
                        PathArg::Headset => AudioPath::Headset,
                        PathArg::Speaker => AudioPath::Speaker,
                    });
                    Ok(String::new())
                }
                AudioAction::Tone { tone } => svc.available().map(|()| {
                    svc.tone(match tone {
                        ToneArg::Dial => Some(Tone::Dial),
                        ToneArg::Ringback => Some(Tone::Ringback),
                        ToneArg::Busy => Some(Tone::Busy),
                        ToneArg::Beep => Some(Tone::Confirm),
                        ToneArg::Off => None,
                    });
                    String::new()
                }),
                AudioAction::Ring { name } if name.as_deref() == Some("off") => {
                    svc.ring(None);
                    Ok(String::new())
                }
                AudioAction::Ring { name } => svc.available().map(|()| {
                    svc.ring(Some(name.as_deref().unwrap_or(super::ringtones::DEFAULT)));
                    String::new()
                }),
                AudioAction::Volume { target, level } => {
                    let target = match target {
                        VolumeArg::Handset => VolumeTarget::Handset,
                        VolumeArg::Headset => VolumeTarget::Headset,
                        VolumeArg::Speaker => VolumeTarget::Speaker,
                        VolumeArg::Ringer => VolumeTarget::Ringer,
                        VolumeArg::Current => VolumeTarget::Current,
                    };
                    let _ = svc.set_volume(target, level);
                    Ok(String::new())
                }
                AudioAction::Mute { state } => {
                    svc.command(Command::SetMute { on: state.is_on() }).map(|_| String::new()).map_err(|e| format!("{e:?}"))
                }
            };
            match result {
                Ok(text) => {
                    if !text.is_empty() {
                        out.out(&text).await;
                    }
                    0
                }
                Err(e) => {
                    out.err(&format!("dphone audio: {e}\n")).await;
                    1
                }
            }
        }
    });

    let sim = ctx.sim;
    ctx.diag.register(move |args: DspCommand, out: Output| {
        let service = service.clone();
        async move {
            // What a subcommand that talks to the DSP made of it.
            let report = |r: Result<String, String>| {
                let out = out.clone();
                async move {
                    match r {
                        Ok(text) => {
                            out.out(&text).await;
                            0
                        }
                        Err(e) => {
                            out.err(&format!("dphone dsp: {e}\n")).await;
                            1
                        }
                    }
                }
            };
            match args.action.unwrap_or(DspAction::Status) {
                DspAction::Status => {
                    let mut text = match service.available() {
                        Ok(()) => "audio engine: running\n".to_string(),
                        Err(e) => format!("audio engine: NOT running: {e}\n"),
                    };
                    if !sim {
                        match opendeskphone_vpm::remoteproc::Remoteproc::find() {
                            Ok(r) => text.push_str(&format!(
                                "remoteproc: {} state {:?} firmware {}\n",
                                r.dir().display(),
                                r.state(),
                                r.firmware().unwrap_or_default().trim()
                            )),
                            Err(e) => text.push_str(&format!("remoteproc: {e}\n")),
                        }
                        match opendeskphone_vpm::Device::open_default().and_then(opendeskphone_vpm::Vpm::new) {
                            Ok(vpm) => {
                                let i = vpm.info();
                                text.push_str(&format!(
                                    "driver: {} jobs, {} timeouts, {} assertions, {} asynchronous events, {} stale results\n",
                                    i.jobs, i.timeouts, i.asserts, i.async_events, i.stale_results
                                ));
                            }
                            Err(e) => text.push_str(&format!("driver: {e}\n")),
                        }
                    }
                    out.out(&text).await;
                    0
                }
                DspAction::Selftest => {
                    let report = opendeskphone_vpm::diag::selftest();
                    let mut text = format!("{} checks, {} failures\n", report.checks, report.failures.len());
                    for f in &report.failures {
                        text.push_str(&format!("FAIL: {f}\n"));
                    }
                    out.out(&text).await;
                    if report.passed() { 0 } else { 1 }
                }
                DspAction::Job { client, channel, instance, op, arg1, arg2, body_bytes, fill_word } => {
                    let fill = fill_word.to_le_bytes();
                    let body: Vec<u8> = (0..body_bytes as usize).map(|i| fill[i % 2]).collect();
                    let r = contained("dsp-job", Duration::from_secs(10), move || {
                        let dev = opendeskphone_vpm::Device::open_default().map_err(|e| e.to_string())?;
                        let mut vpm = opendeskphone_vpm::Vpm::new(dev).map_err(|e| e.to_string())?;
                        Ok::<_, String>(opendeskphone_vpm::diag::probe_job(&mut vpm, client, channel, instance, op, arg1, arg2, &body))
                    })
                    .await
                    .and_then(|r| r);
                    report(r.map(|text| format!("{text}\n"))).await
                }
                // The routing experiments of `opendeskphone-media`: a test
                // signal through the filter bank, or through one canceller
                // with the signal in one band buffer.
                action @ (DspAction::Bands { .. } | DspAction::Canceller { .. }) => {
                    use opendeskphone_vpm::diag::{Canceller, Signal};
                    // Like `dsp job`, this sends jobs on a handle of its own.
                    let path = service.store.get::<Audio>().path;
                    if path != AudioPath::None {
                        out.out(&format!("note: the audio path is open ({path:?}); the engine's own jobs run between these\n")).await;
                    }
                    let r = contained("dsp-experiment", Duration::from_secs(60), move || {
                        let dev = opendeskphone_vpm::Device::open_default().map_err(|e| e.to_string())?;
                        let mut vpm = opendeskphone_vpm::Vpm::new(dev).map_err(|e| e.to_string())?;
                        match action {
                            DspAction::Bands { stream, tone, frames, .. } => {
                                let signal = tone.map_or(Signal::Sweep, Signal::Tone);
                                opendeskphone_vpm::diag::band_experiment(&mut vpm, stream as usize, signal, frames.frames as usize)
                                    .map(|r| r.to_string())
                                    .map_err(|e| e.to_string())
                            }
                            DspAction::Canceller { canceller, direction, buffer, hz, frames } => {
                                let which = match canceller {
                                    CancellerArg::Hec0 => Canceller::Hec(0),
                                    CancellerArg::Hec1 => Canceller::Hec(1),
                                    CancellerArg::Sbaec => Canceller::Sbaec,
                                };
                                let ingress = matches!(direction, DirectionArg::Ingress);
                                opendeskphone_vpm::diag::canceller_experiment(&mut vpm, which, ingress, buffer, hz, frames.frames as usize)
                                    .map(|r| r.to_string())
                            }
                            _ => unreachable!("only the experiments get here"),
                        }
                    })
                    .await
                    .and_then(|r| r);
                    report(r).await
                }
                DspAction::Restart => {
                    // The engine must not reach the DSP between its reload
                    // and the start-up sequence; give the audio thread a
                    // few ticks to take the hold.
                    let _ = service.inner.lock().unwrap().send(Ec::HoldDsp);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let r = contained("dsp-restart", Duration::from_secs(30), || {
                        opendeskphone_vpm::remoteproc::Remoteproc::find().and_then(|r| r.restart()).map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|r| r);
                    if r.is_ok() {
                        let _ = service.inner.lock().unwrap().send(Ec::RestartDsp);
                    }
                    report(r.map(|()| "DSP restarted\n".to_string())).await
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_file_formats() {
        // Headerless samples, as stock's `.sln16` files are; an odd last
        // byte is dropped.
        assert_eq!(decode_sound(&[0x20, 0x03, 0x9d, 0x07, 0xff]).unwrap(), [0x0320, 0x079d]);
        // A WAVE file is decoded by its header.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\x04\0\0\0");
        wav.extend_from_slice(&[1, 0, 0xff, 0xff]);
        assert_eq!(decode_sound(&wav).unwrap(), [1, -1]);
        assert!(decode_sound(b"RIFF....nope").is_err());
    }

    #[test]
    fn gains() {
        assert_eq!(rx_gain_q8(0, 5), 0);
        assert_eq!(rx_gain_q8(-768, 6), 0);
        assert_eq!(rx_gain_q8(0, 0), -15 * 256);
        assert_eq!(ring_gain_q12(5), 4096);
        assert_eq!(ring_gain_q12(0), 0);
        assert!(ring_gain_q12(10) > ring_gain_q12(9));
    }
}

/// The volume keys on the ringer's volume: the ringer sounds on an idle
/// phone and only there, and stops by itself or when something happens.
/// (No engine runs here: these check what the service decides, and
/// `opendeskphone-media`'s engine tests check that a sounding ring takes a new
/// gain without starting again.)
#[cfg(test)]
mod volume_key_tests {
    use super::*;
    use opendeskphone_state::{AccountId, Call, CallState, Calls, Direction, Event, InputSource, Key, Model, Party};

    fn service() -> AudioService {
        AudioService::new(Store::new(Model::D60), opendeskphone_vpm::Family::New)
    }

    fn key(svc: &AudioService, delta: i8) {
        svc.command(Command::AdjustVolume { target: VolumeTarget::Current, delta }).unwrap();
    }

    /// The volume keys' preview in progress, if any.
    fn preview(svc: &AudioService) -> Option<u64> {
        let inner = svc.inner.lock().unwrap();
        (inner.preview != 0 && inner.preview_is_volume).then_some(inner.preview)
    }

    /// (the ringer sounds, the open path) as the rest of the phone sees them.
    fn sounding(svc: &AudioService) -> (bool, AudioPath) {
        assert_eq!(svc.inner.lock().unwrap().ringing, svc.store.get::<Audio>().ringing);
        svc.store.read::<Audio, _>(|a| (a.ringing, a.path))
    }

    fn add_call(svc: &AudioService, state: CallState) -> CallId {
        let id = svc.store.next_call_id();
        svc.store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party::default());
            call.state = state;
            c.calls.push(call);
        });
        id
    }

    #[tokio::test]
    async fn the_ringer_sounds_at_the_new_volume_on_an_idle_phone() {
        let svc = service();
        assert_eq!(sounding(&svc), (false, AudioPath::None));
        key(&svc, 1);
        assert_eq!(svc.store.get::<Audio>().ringer_volume, 6);
        assert_eq!(sounding(&svc), (true, AudioPath::Speaker), "the ringer, on its own output");
        let first = preview(&svc).expect("a preview");
        // It is the ringer's volume that the popup shows and that the
        // next key adjusts, though the speaker is what is open.
        let a = svc.store.get::<Audio>();
        assert_eq!((a.current_target(), a.volume(VolumeTarget::Current)), (VolumeTarget::Ringer, 6));
        // No lamp for it, as for a ring.
        assert_eq!(opendeskphone_state::audio_lamps(&a, &svc.store.get::<Calls>()).0, opendeskphone_state::LampState::OFF);

        // Another key: the same ring goes on, and the time starts again.
        key(&svc, 1);
        let second = preview(&svc).expect("still a preview");
        assert_ne!(first, second);
        let a = svc.store.get::<Audio>();
        assert_eq!((a.ringer_volume, a.speaker_volume, a.ringing), (7, 5, true));
        svc.end_volume_preview(Some(first));
        assert_eq!(sounding(&svc), (true, AudioPath::Speaker), "the first key's time is not the preview's any more");
        svc.end_volume_preview(Some(second));
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::None), None));

        // At a limit the volume stays and the ringer still sounds; at 0
        // it "sounds" at no gain.
        svc.store.update::<Audio, _>(|a| a.ringer_volume = VOLUME_MAX);
        key(&svc, 1);
        assert_eq!((svc.store.get::<Audio>().ringer_volume, sounding(&svc).0), (VOLUME_MAX, true));
        svc.end_volume_preview(None);
        svc.store.update::<Audio, _>(|a| a.ringer_volume = 1);
        key(&svc, -1);
        assert_eq!((svc.store.get::<Audio>().ringer_volume, sounding(&svc).0), (0, true));
        svc.end_volume_preview(None);
        assert_eq!(sounding(&svc), (false, AudioPath::None));

        // Setting the ringer's volume some other way (the menu, the web
        // UI) is not the volume keys.
        svc.command(Command::SetVolume { target: VolumeTarget::Ringer, level: 4 }).unwrap();
        svc.command(Command::AdjustVolume { target: VolumeTarget::Ringer, delta: 1 }).unwrap();
        assert_eq!((svc.store.get::<Audio>().ringer_volume, sounding(&svc).0), (5, false));
    }

    #[tokio::test]
    async fn it_stops_by_itself() {
        let svc = service();
        key(&svc, -1);
        assert!(sounding(&svc).0);
        tokio::time::sleep(VOLUME_PREVIEW / 2).await;
        key(&svc, -1);
        tokio::time::sleep(VOLUME_PREVIEW * 3 / 4).await;
        assert!(sounding(&svc).0, "the time runs from the last key");
        tokio::time::sleep(VOLUME_PREVIEW / 2).await;
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::None), None));
        assert_eq!(svc.store.get::<Audio>().ringer_volume, 3);
    }

    #[tokio::test]
    async fn not_during_a_call() {
        // A call on the handset: the keys are the handset's.
        let svc = service();
        add_call(&svc, CallState::Active);
        svc.request_path(AudioPath::Handset);
        key(&svc, 1);
        let a = svc.store.get::<Audio>();
        assert_eq!((a.handset_volume, a.ringer_volume), (6, 5));
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::Handset), None));

        // A call with no path open (on hold): no ringer over it.
        let svc = service();
        add_call(&svc, CallState::Held { local: true, remote: false });
        key(&svc, 1);
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::None), None));

        // Dial tone on the speaker: the keys are the speaker's.
        let svc = service();
        svc.tone(Some(Tone::Dial));
        key(&svc, 1);
        let a = svc.store.get::<Audio>();
        assert_eq!((a.speaker_volume, a.ringer_volume), (6, 5));
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::Speaker), None));

        // The handset off its hook with nothing sounding.
        let svc = service();
        svc.input_event(&Event::Hook { off_hook: true, source: InputSource::Physical });
        key(&svc, 1);
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::None), None));
    }

    #[tokio::test]
    async fn not_while_ringing_for_a_call() {
        let svc = service();
        add_call(&svc, CallState::Ringing);
        svc.ring(Some("Bell Phone"));
        key(&svc, 1);
        // The ringer's volume changed, and the ring is the call's still:
        // nothing ends it but call control.
        assert_eq!(svc.store.get::<Audio>().ringer_volume, 6);
        assert_eq!((sounding(&svc), preview(&svc)), ((true, AudioPath::Speaker), None));
        svc.end_volume_preview(None);
        svc.input_event(&Event::Key { key: Key::Digit(5), down: true, source: InputSource::Physical });
        assert_eq!(sounding(&svc), (true, AudioPath::Speaker));
        svc.ring(None);
        assert_eq!(sounding(&svc), (false, AudioPath::None));
    }

    #[tokio::test]
    async fn the_handset_another_key_or_a_tone_stops_it() {
        let svc = service();
        let start = |svc: &AudioService| {
            key(svc, 1);
            assert_eq!(sounding(svc), (true, AudioPath::Speaker));
        };
        let stopped = |svc: &AudioService, what: &str| {
            assert_eq!((sounding(svc).0, preview(svc)), (false, None), "{what}");
        };

        start(&svc);
        svc.input_event(&Event::Hook { off_hook: true, source: InputSource::Physical });
        stopped(&svc, "the handset lifted");
        assert_eq!(sounding(&svc).1, AudioPath::None, "the ringer's speaker is not a path to move to the handset");
        assert!(svc.store.get::<Audio>().off_hook);
        svc.input_event(&Event::Hook { off_hook: false, source: InputSource::Physical });

        // The volume keys themselves and key releases are not "another key".
        start(&svc);
        for (key, down) in [(Key::VolumeUp, true), (Key::VolumeDown, true), (Key::VolumeUp, false), (Key::Digit(5), false)] {
            svc.input_event(&Event::Key { key, down, source: InputSource::Physical });
            assert!(preview(&svc).is_some(), "{key:?} {down}");
        }
        svc.input_event(&Event::Key { key: Key::Digit(5), down: true, source: InputSource::Physical });
        stopped(&svc, "another key");

        start(&svc);
        svc.tone(Some(Tone::Dial));
        stopped(&svc, "a tone");
        assert_eq!(svc.store.get::<Audio>().tone, Some(Tone::Dial));
        svc.tone(None);

        start(&svc);
        svc.request_path(AudioPath::Speaker);
        stopped(&svc, "the speaker key");
        svc.request_path(AudioPath::None);
        assert_eq!(sounding(&svc), (false, AudioPath::None));
    }

    #[tokio::test]
    async fn an_arriving_call_takes_the_ringer() {
        // Call control reserves the call's port first.
        let svc = service();
        key(&svc, 1);
        let mine = preview(&svc).unwrap();
        let call = add_call(&svc, CallState::Ringing);
        let _ = svc.open(call);
        assert_eq!((sounding(&svc), preview(&svc)), ((false, AudioPath::None), None));
        svc.ring(Some("Bell Phone"));
        // The preview's time coming up, or another key, does not stop
        // the call's ring.
        svc.end_volume_preview(Some(mine));
        svc.end_volume_preview(None);
        assert_eq!((sounding(&svc), preview(&svc)), ((true, AudioPath::Speaker), None));

        // The ring itself coming first does the same.
        let svc = service();
        key(&svc, 1);
        let mine = preview(&svc).unwrap();
        svc.ring(Some("Bell Phone"));
        assert_eq!(preview(&svc), None);
        svc.end_volume_preview(Some(mine));
        assert_eq!(sounding(&svc), (true, AudioPath::Speaker));
    }

    #[tokio::test]
    async fn a_ringtone_tried_in_the_settings_is_left_alone() {
        let svc = service();
        svc.command(Command::PlaySound { name: "Bell Phone".into() }).unwrap();
        assert!(sounding(&svc).0);
        key(&svc, 1);
        // Its volume changes; it stays the settings' preview, which
        // another key does not end.
        assert_eq!(svc.store.get::<Audio>().ringer_volume, 6);
        assert_eq!(preview(&svc), None);
        svc.input_event(&Event::Key { key: Key::Down, down: true, source: InputSource::Physical });
        assert!(sounding(&svc).0);
        svc.command(Command::StopSound).unwrap();
        assert_eq!(sounding(&svc), (false, AudioPath::None));
    }
}

/// A ringtone tried in the settings is heard whole, and ends where its
/// sound does.
#[cfg(test)]
mod ringtone_preview_tests {
    use super::*;
    use opendeskphone_state::Model;

    fn sound(ms: usize) -> Vec<i16> {
        vec![1000; ms * 16]
    }

    fn silence(ms: usize) -> Vec<i16> {
        vec![0; ms * 16]
    }

    /// What the preview of a ringtone of this pass and pause plays.
    fn preview(pass_ms: usize, pause_ms: u32) -> Vec<i16> {
        match ringtone_preview(RingSource::Pcm { samples: sound(pass_ms).into(), pause_ms }, 7) {
            RingSource::Once { samples, start: 0, token: 7 } => samples.to_vec(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_long_ringtone_is_heard_whole() {
        assert_eq!(preview(31_000, 500), sound(31_000));
    }

    #[test]
    fn a_short_ringtone_is_heard_in_the_whole_passes_that_begin_within_four_seconds() {
        // Passes begin at 0 and 3 s; the second plays on to its end.
        assert_eq!(preview(2000, 1000), [sound(2000), silence(1000), sound(2000)].concat());
        // At 0, 1.5, and 3 s.
        assert_eq!(preview(1000, 500), [sound(1000), silence(500), sound(1000), silence(500), sound(1000)].concat());
    }

    #[test]
    fn it_ends_when_its_sound_has_played_out() {
        let svc = AudioService::new(Store::new(Model::D60), opendeskphone_vpm::Family::New);
        let sounding = |svc: &AudioService| svc.store.read::<Audio, _>(|a| (a.ringing, a.path));
        svc.command(Command::PlaySound { name: "Crazy Dream".into() }).unwrap();
        assert_eq!(sounding(&svc), (true, AudioPath::Speaker));
        let token = svc.inner.lock().unwrap().preview_sound.expect("a ringtone preview");
        svc.sound_ended(token, 0);
        assert_eq!(sounding(&svc), (false, AudioPath::None));
    }
}

#[cfg(test)]
mod bluetooth_rate_tests {
    use super::{narrow, widen};

    #[test]
    fn eight_kilohertz_and_back() {
        // Up by four: a ramp from the previous sample to each new one.
        let mut last = 0i16;
        let mut out = [0i16; 8];
        widen(&[400, -400], &mut last, 4, &mut out);
        assert_eq!(out, [100, 200, 300, 400, 200, 0, -200, -400]);
        assert_eq!(last, -400);
        // Down by four: the mean of each four.
        let mut back = Vec::new();
        narrow(&out, 4, &mut back);
        assert_eq!(back, [250, -100]);
        // A factor of one changes nothing.
        let mut same = [0i16; 2];
        widen(&[7, 9], &mut 0, 1, &mut same);
        assert_eq!(same, [7, 9]);
    }
}
