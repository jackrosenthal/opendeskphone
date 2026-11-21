// SPDX-License-Identifier: GPL-2.0-or-later
//! The real-time audio engine.
//!
//! One thread owns the audio device, the DSP handle, and the media sockets.
//! It runs on the capture clock in 10 ms ticks (two 5 ms capture periods):
//!
//! 1. read 10 ms from the microphone (this is the clock);
//! 2. receive RTP, feed the jitter buffers, report DTMF events;
//! 3. per stream: take 10 ms from the jitter buffer and decode it
//!    (G.711 here, anything else as DSP jobs) into band buffers;
//! 4. mix the streams, tones, and ringing; canceller egress; convert to PCM
//!    and play;
//! 5. microphone to band buffers; canceller ingress;
//! 6. per stream: microphone (plus the other conference legs) to the
//!    encoder, packetize, protect, send.
//!
//! That is the order the stock firmware's jobs were seen in (decode,
//! egress, ingress, encode).  Everything is integer arithmetic, and after a
//! stream has started nothing in the tick allocates.
//!
//! [`EngineCore`] is the engine without its thread: tests and the host
//! simulator call [`EngineCore::tick`] directly.  [`Engine`] runs it on a
//! `SCHED_FIFO` thread behind a bounded command channel.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use opendeskphone_vpm::Codec;

use crate::bands::{BandFrame, NB, SWB, Split2, WB, mix_into, scale};
use crate::device::{AudioDevice, AudioPath, Dsp, GainControl};
use crate::dtmf::{DtmfEdge, DtmfReceiver, DtmfSender, EventPayload};
use crate::jitter::{JitterBuffer, JitterStats, Pop, UNIT_MAX};
use crate::ring::{RingPlayer, RingSource};
use crate::rtp::{self, RtcpItem, RtpHeader, RxStats, SenderInfo, TS_PER_TICK};
use crate::srtp::{MasterKey, SRTCP_OVERHEAD, SRTP_MAX_OVERHEAD, SrtpContext, Suite};
use crate::tone::{ToneGen, ToneSpec};

/// Voice streams: the DSP has three (the stock firmware opens three).  A
/// conference is therefore at most three legs.
pub const MAX_STREAMS: usize = 3;
/// Ticks between RTCP reports and statistics events (5 s).
const REPORT_TICKS: u32 = 500;
const MAX_PACKET: usize = 1500;
/// Largest payload of one packet: 60 ms.
const MAX_UNITS: usize = 6;

/// The two companding laws of G.711.  G.711 runs on the ARM on every model,
/// because the DSP has no G.711 job (the stock firmware codes it host-side
/// as well).  The coding itself is the `audio-codec` crate's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Law {
    Mu,
    A,
}

impl Law {
    /// One sample for each byte of `input`; `out` is at least as long.
    pub fn decode(self, input: &[u8], out: &mut [i16]) {
        use audio_codec::Decoder;
        let n = input.len().min(out.len());
        // Neither decoder keeps anything between calls, and neither fails
        // when the output is as long as the input.
        let _ = match self {
            Law::Mu => audio_codec::pcmu::PcmuDecoder::new().decode_into(&input[..n], out),
            Law::A => audio_codec::pcma::PcmaDecoder::new().decode_into(&input[..n], out),
        };
    }

    /// One byte for each sample of `input`; `out` is at least as long.
    pub fn encode(self, input: &[i16], out: &mut [u8]) {
        use audio_codec::Encoder;
        let n = input.len().min(out.len());
        let _ = match self {
            Law::Mu => audio_codec::pcmu::PcmuEncoder::new().encode_into(&input[..n], out),
            Law::A => audio_codec::pcma::PcmaEncoder::new().encode_into(&input[..n], out),
        };
    }

    /// The byte that decodes to silence.
    pub fn silence(self) -> u8 {
        match self {
            Law::Mu => 0xff,
            Law::A => 0xd5,
        }
    }
}

/// Codecs a stream can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PayloadCodec {
    Pcmu,
    Pcma,
    G722,
    G726_32,
}

impl PayloadCodec {
    /// Payload bytes per 10 ms.
    pub fn unit_bytes(self) -> usize {
        match self {
            PayloadCodec::Pcmu | PayloadCodec::Pcma | PayloadCodec::G722 => 80,
            PayloadCodec::G726_32 => 40,
        }
    }

    /// The codec's DSP job codec; `None` for G.711, which runs here.
    pub fn dsp_codec(self) -> Option<Codec> {
        match self {
            PayloadCodec::Pcmu | PayloadCodec::Pcma => None,
            PayloadCodec::G722 => Some(Codec::G722),
            PayloadCodec::G726_32 => Some(Codec::G726_32),
        }
    }

    /// Static payload type, where RFC 3551 assigns one.
    pub fn static_payload_type(self) -> Option<u8> {
        match self {
            PayloadCodec::Pcmu => Some(0),
            PayloadCodec::Pcma => Some(8),
            PayloadCodec::G722 => Some(9),
            PayloadCodec::G726_32 => None,
        }
    }

    /// The name and clock rate for an SDP `rtpmap` line.
    pub fn rtpmap(self) -> &'static str {
        match self {
            PayloadCodec::Pcmu => "PCMU/8000",
            PayloadCodec::Pcma => "PCMA/8000",
            PayloadCodec::G722 => "G722/8000",
            PayloadCodec::G726_32 => "G726-32/8000",
        }
    }

    /// Whether this codec can be used on `family`'s DSP as it is understood
    /// today (call setup offers only these).
    pub fn is_available(self, family: opendeskphone_vpm::Family) -> bool {
        self.dsp_codec().is_none_or(|c| c.is_supported(family))
    }

    pub const ALL: [PayloadCodec; 4] = [PayloadCodec::G722, PayloadCodec::Pcmu, PayloadCodec::Pcma, PayloadCodec::G726_32];
}

#[derive(Clone, Debug)]
pub struct SrtpConfig {
    pub suite: Suite,
    /// Our key (the one we offered or answered with).
    pub tx: MasterKey,
    /// The peer's key.
    pub rx: MasterKey,
}

/// Everything a stream needs.  Sockets are bound by the caller (the port is
/// in the SDP before the stream exists) and handed over.
#[derive(Debug)]
pub struct StreamConfig {
    pub codec: PayloadCodec,
    pub payload_type: u8,
    /// Packet time in ms: 10 to 60 in steps of 10.
    pub ptime_ms: u8,
    pub rtp: UdpSocket,
    pub rtcp: Option<UdpSocket>,
    pub remote_rtp: SocketAddr,
    pub remote_rtcp: Option<SocketAddr>,
    /// Payload type of RFC 4733 events, when negotiated.  Without it DTMF
    /// is sent as tones in the audio.
    pub dtmf_payload_type: Option<u8>,
    pub srtp: Option<SrtpConfig>,
    pub send: bool,
    pub recv: bool,
    /// Send to wherever valid packets come from (NAT).
    pub symmetric: bool,
    /// Report when no RTP arrives for this long while receiving; 0 = never.
    pub rtp_timeout_ms: u32,
    pub jitter_min_ms: u32,
    pub jitter_max_ms: u32,
    /// CNAME for RTCP.
    pub cname: String,
}

impl StreamConfig {
    /// A two-way stream with the usual defaults.
    pub fn new(codec: PayloadCodec, payload_type: u8, rtp: UdpSocket, remote_rtp: SocketAddr) -> Self {
        Self {
            codec,
            payload_type,
            ptime_ms: 20,
            rtp,
            rtcp: None,
            remote_rtp,
            remote_rtcp: None,
            dtmf_payload_type: None,
            srtp: None,
            send: true,
            recv: true,
            symmetric: true,
            rtp_timeout_ms: 30_000,
            jitter_min_ms: 20,
            jitter_max_ms: 200,
            cname: String::from("opendeskphone"),
        }
    }
}

/// How to send a DTMF digit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DtmfMode {
    /// RFC 4733 events if negotiated, otherwise in-band tones.
    Auto,
    /// Tones in the audio.
    Inband,
}

#[derive(Debug)]
pub enum Command {
    StartStream { id: usize, config: Box<StreamConfig> },
    StopStream { id: usize },
    /// Hold and resume, and one-way streams.
    SetDirection { id: usize, send: bool, recv: bool },
    /// Re-point a stream (re-INVITE with a new address).
    SetRemote { id: usize, rtp: SocketAddr, rtcp: Option<SocketAddr> },
    /// Streams (bit per id) that hear each other besides the phone's user.
    SetConference { mask: u8 },
    /// Opens the audio device on a path (and its canceller), or closes it.
    SetPath(Option<AudioPath>),
    /// Receive gain of a path in dB * 256, applied by the canceller.
    SetRxGain { path: AudioPath, gain_q8: i16 },
    /// A front-end mixer control.
    SetDeviceGain { path: AudioPath, control: GainControl, value: i32 },
    MuteMic(bool),
    /// Plays a tone to the user (replacing any other); `None` stops it.
    PlayTone(Option<ToneSpec>),
    /// Rings (replacing any other ring); `None` stops.  `gain_q12` scales
    /// the ring (4096 = unity).  A [`RingSource::Once`] source is a sound
    /// file: it plays once, and [`Event::SoundEnded`] says where it ended
    /// or was replaced.
    PlayRing(Option<RingSource>, i32),
    /// Changes the gain of the ring that is sounding (4096 = unity); the
    /// ring goes on from where it is.  Nothing if there is no ring.
    SetRingGain(i32),
    /// Sends a digit on a stream, optionally with a tone to the user.
    SendDtmf { id: usize, key: char, duration_ms: u32, mode: DtmfMode, feedback: bool },
    /// The DSP is about to be restarted: send it nothing until
    /// [`Command::RestartDsp`].  A freshly loaded DSP that gets frame jobs
    /// before its start-up sequence does not answer the sequence's Init
    /// jobs afterward (seen on a D60).
    HoldDsp,
    /// The DSP was restarted: repeat the start-up and re-open everything.
    RestartDsp,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub packets_sent: u32,
    pub octets_sent: u32,
    pub packets_received: u32,
    pub packets_lost: i32,
    /// Interarrival jitter in ms.
    pub jitter_ms: u32,
    pub jitter_buffer: JitterStats,
    /// Packets that failed SRTP checks or could not be parsed.
    pub rejected: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A digit from the far end began or ended.
    Dtmf { id: usize, edge: DtmfEdge },
    /// No RTP for the configured time.
    RtpTimeout { id: usize },
    Stats { id: usize, stats: StreamStats },
    /// A stream could not be started or failed for good.
    StreamError { id: usize, error: String },
    /// A DSP job failed.  After a fatal one (`needs_restart`), restart the
    /// DSP and send [`Command::RestartDsp`].
    DspError { error: opendeskphone_vpm::Error, needs_restart: bool },
    /// The DSP start-up configuration was sent; `summary` says how it went.
    DspStartup { summary: String },
    AudioError { kind: io::ErrorKind },
    /// The audio thread asked for `SCHED_FIFO` at `priority` and the kernel
    /// refused with this error number; the thread runs on at its ordinary
    /// priority.
    RealtimeRefused { priority: i32, errno: i32 },
    /// A non-repeating tone finished.
    ToneEnded,
    /// A [`RingSource::Once`] sound played to its end, or was stopped or
    /// replaced; `position` is the 16 kHz sample it reached.
    SoundEnded { token: u32, position: usize },
}

type EventSink = Box<dyn FnMut(Event) + Send>;

struct Stream {
    codec: PayloadCodec,
    law: Option<Law>,
    payload_type: u8,
    dtmf_pt: Option<u8>,
    units_per_packet: usize,
    rtp: UdpSocket,
    rtcp: Option<UdpSocket>,
    remote_rtp: SocketAddr,
    remote_rtcp: Option<SocketAddr>,
    symmetric: bool,
    send: bool,
    recv: bool,
    cname: String,

    ssrc: u32,
    seq: u16,
    ts: u32,
    marker: bool,
    tx: [u8; rtp::RTP_HEADER + MAX_UNITS * UNIT_MAX + SRTP_MAX_OVERHEAD],
    tx_units: usize,
    packets_sent: u32,
    octets_sent: u32,
    sent_since_report: bool,

    jitter: JitterBuffer,
    rx_stats: RxStats,
    rejected: u32,
    last_rx_tick: u32,
    timeout_ticks: u32,
    timed_out: bool,
    /// What this stream contributes this tick.
    rx_bands: BandFrame,
    plc_last: [i16; NB],
    plc_run: u32,

    dtmf_rx: DtmfReceiver,
    dtmf_tx: Option<DtmfSender>,
    dtmf_queue: VecDeque<(char, u32, DtmfMode)>,
    dtmf_gap: u32,
    inband: Option<(ToneGen, u32)>,

    srtp_tx: Option<SrtpContext>,
    srtp_rx: Option<SrtpContext>,
    dsp_ready: bool,
}

/// The engine without its thread.
/// Signal levels over the engine's last full second, for diagnostics
/// ("is anything being played or picked up?").  Sample values are absolute
/// 16-bit magnitudes.
#[derive(Debug, Default)]
pub struct Meters {
    /// Ticks run since the engine started.
    pub ticks: AtomicU32,
    /// Loudest sample written to the audio device.
    pub speaker_peak: AtomicU32,
    /// Mean magnitude of the samples written to the audio device.
    pub speaker_mean: AtomicU32,
    /// Loudest sample read from the audio device.
    pub mic_peak: AtomicU32,
    /// Mean magnitude of the samples read from the audio device.
    pub mic_mean: AtomicU32,
    /// DSP job failures since the engine started.
    pub dsp_errors: AtomicU32,
}

#[derive(Default)]
struct MeterWindow {
    spk_peak: u32,
    spk_sum: u64,
    mic_peak: u32,
    mic_sum: u64,
    samples: u64,
}

impl MeterWindow {
    fn add(&mut self, spk: &[i16], mic: &[i16]) {
        for v in spk {
            let a = v.unsigned_abs() as u32;
            self.spk_peak = self.spk_peak.max(a);
            self.spk_sum += a as u64;
        }
        for v in mic {
            let a = v.unsigned_abs() as u32;
            self.mic_peak = self.mic_peak.max(a);
            self.mic_sum += a as u64;
        }
        self.samples += spk.len() as u64;
    }
}

pub struct EngineCore {
    audio: Box<dyn AudioDevice>,
    dsp: Box<dyn Dsp>,
    events: EventSink,
    rate: u32,
    streams: [Option<Box<Stream>>; MAX_STREAMS],
    conference: u8,
    path: Option<AudioPath>,
    device_open: bool,
    rx_gain: [i16; 3],
    /// The tone mode last given to the attached path's canceller.
    tone_mode: bool,
    mute: bool,
    tone: Option<ToneGen>,
    feedback: Option<(ToneGen, u32)>,
    ring: Option<RingPlayer>,
    ring_split: Split2,
    tick: u32,
    dsp_failed: bool,
    dsp_errors: u32,
    audio_errors: u32,
    // scratch, so that the tick does not allocate
    mic_pcm: [i16; SWB],
    spk_pcm: [i16; SWB],
    meters: Arc<Meters>,
    window: MeterWindow,
    mic: BandFrame,
    far: BandFrame,
    scratch: BandFrame,
    pkt: [u8; MAX_PACKET],
    rtcp_buf: Vec<u8>,
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    // A failing random source only makes SSRCs predictable.
    let _ = getrandom::getrandom(&mut b);
    u32::from_ne_bytes(b)
}

impl EngineCore {
    pub fn new(audio: Box<dyn AudioDevice>, dsp: Box<dyn Dsp>, events: EventSink) -> Self {
        let rate = audio.rate();
        let gains = opendeskphone_vpm::startup::tuning(dsp.family()).gains;
        Self {
            audio,
            dsp,
            events,
            rate,
            streams: [None, None, None],
            conference: 0,
            path: None,
            device_open: false,
            rx_gain: [gains.handset, gains.headset, gains.speaker],
            tone_mode: false,
            mute: false,
            tone: None,
            feedback: None,
            ring: None,
            ring_split: Split2::new(),
            tick: 0,
            dsp_failed: false,
            dsp_errors: 0,
            audio_errors: 0,
            mic_pcm: [0; SWB],
            spk_pcm: [0; SWB],
            meters: Arc::new(Meters::default()),
            window: MeterWindow::default(),
            mic: BandFrame::ZERO,
            far: BandFrame::ZERO,
            scratch: BandFrame::ZERO,
            pkt: [0; MAX_PACKET],
            rtcp_buf: Vec::with_capacity(256),
        }
    }

    /// Sends the DSP its start-up configuration.  Call once before ticking.
    pub fn start(&mut self) {
        let r = self.dsp.start(false);
        self.report_startup();
        if let Err(e) = r {
            self.dsp_error(e);
        }
    }

    fn report_startup(&mut self) {
        if let Some(text) = self.dsp.startup_summary() {
            self.emit(Event::DspStartup { summary: text });
        }
    }

    /// Whether anything needs ticks: an open device, a stream, or a sound.
    pub fn is_active(&self) -> bool {
        self.device_open || self.streams.iter().any(Option::is_some) || self.tone.is_some() || self.ring.is_some()
    }

    pub fn ticks(&self) -> u32 {
        self.tick
    }

    /// The level meters; see [`Meters`].
    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
    }

    fn meter(&mut self, n: usize, device: bool) {
        if device {
            let (spk, mic) = (&self.spk_pcm[..n], &self.mic_pcm[..n]);
            self.window.add(spk, mic);
        }
        self.meters.ticks.store(self.tick, Ordering::Relaxed);
        if self.tick.is_multiple_of(100) {
            let w = std::mem::take(&mut self.window);
            let mean = |sum: u64| sum.checked_div(w.samples).unwrap_or(0) as u32;
            self.meters.speaker_peak.store(w.spk_peak, Ordering::Relaxed);
            self.meters.speaker_mean.store(mean(w.spk_sum), Ordering::Relaxed);
            self.meters.mic_peak.store(w.mic_peak, Ordering::Relaxed);
            self.meters.mic_mean.store(mean(w.mic_sum), Ordering::Relaxed);
        }
    }

    fn emit(&mut self, e: Event) {
        (self.events)(e);
    }

    fn dsp_error(&mut self, e: opendeskphone_vpm::Error) {
        self.dsp_errors += 1;
        self.meters.dsp_errors.fetch_add(1, Ordering::Relaxed);
        let fatal = e.is_fatal();
        // one report per failure burst, not one per tick
        if (fatal && !self.dsp_failed) || (!fatal && self.dsp_errors % 100 == 1) {
            self.emit(Event::DspError { error: e, needs_restart: fatal });
        }
        self.dsp_failed |= fatal;
    }

    /// Whether a local tone is playing (the path's canceller is told).
    fn tone_playing(&self) -> bool {
        self.tone.is_some()
    }

    fn attach_dsp(&mut self, path: AudioPath) {
        let tone = self.tone_playing();
        self.tone_mode = tone;
        if let Err(e) = self.dsp.attach(path, self.rx_gain[path.index()], tone) {
            self.dsp_error(e);
        }
    }

    fn open_path(&mut self) {
        let Some(path) = self.path else { return };
        match self.audio.open(path) {
            Ok(()) => self.device_open = true,
            Err(e) => {
                self.device_open = false;
                self.emit(Event::AudioError { kind: e.kind() });
            }
        }
        if !self.dsp_failed {
            self.attach_dsp(path);
        }
    }

    fn close_path(&mut self) {
        if let Some(path) = self.path.take() {
            self.audio.close();
            self.device_open = false;
            if !self.dsp_failed
                && let Err(e) = self.dsp.detach(path) {
                    self.dsp_error(e);
                }
        }
    }

    /// Tell the attached path's canceller when a local tone starts or
    /// stops.
    fn update_tone_mode(&mut self) {
        let tone = self.tone_playing();
        if tone == self.tone_mode {
            return;
        }
        self.tone_mode = tone;
        if let (Some(path), false) = (self.path, self.dsp_failed)
            && let Err(e) = self.dsp.set_tone_mode(path, tone) {
                self.dsp_error(e);
            }
    }

    /// Applies a command.  [`Command::Shutdown`] closes everything.
    pub fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::StartStream { id, config } => self.start_stream(id, *config),
            Command::StopStream { id } => self.stop_stream(id),
            Command::SetDirection { id, send, recv } => {
                if let Some(Some(s)) = self.streams.get_mut(id) {
                    if recv && !s.recv {
                        s.jitter.reset();
                        s.last_rx_tick = self.tick;
                        s.timed_out = false;
                    }
                    if send && !s.send {
                        s.marker = true;
                        s.tx_units = 0;
                    }
                    s.send = send;
                    s.recv = recv;
                }
            }
            Command::SetRemote { id, rtp, rtcp } => {
                if let Some(Some(s)) = self.streams.get_mut(id) {
                    s.remote_rtp = rtp;
                    s.remote_rtcp = rtcp;
                }
            }
            Command::SetConference { mask } => self.conference = mask,
            Command::SetPath(path) => {
                if path != self.path {
                    self.close_path();
                    self.path = path;
                    self.open_path();
                }
            }
            Command::SetRxGain { path, gain_q8 } => {
                self.rx_gain[path.index()] = gain_q8;
                if self.path == Some(path) && !self.dsp_failed
                    && let Err(e) = self.dsp.set_rx_gain(path, gain_q8) {
                        self.dsp_error(e);
                    }
            }
            Command::SetDeviceGain { path, control, value } => {
                if let Err(e) = self.audio.set_gain(path, control, value) {
                    self.emit(Event::AudioError { kind: e.kind() });
                }
            }
            Command::MuteMic(on) => self.mute = on,
            Command::PlayTone(spec) => self.tone = spec.map(ToneGen::new),
            Command::PlayRing(source, gain_q12) => {
                self.end_sound();
                self.ring_split.reset();
                self.ring = source.map(|s| {
                    let mut p = RingPlayer::new(s);
                    p.set_gain_q12(gain_q12);
                    p
                });
            }
            Command::SetRingGain(gain_q12) => {
                if let Some(ring) = &mut self.ring {
                    ring.set_gain_q12(gain_q12);
                }
            }
            Command::SendDtmf { id, key, duration_ms, mode, feedback } => {
                if feedback
                    && let Some(spec) = ToneSpec::dtmf(key, -20) {
                        self.feedback = Some((ToneGen::new(spec), duration_ms.clamp(40, 8000) / 10));
                    }
                if let Some(Some(s)) = self.streams.get_mut(id)
                    && s.dtmf_queue.len() < s.dtmf_queue.capacity() {
                        s.dtmf_queue.push_back((key, duration_ms, mode));
                    }
            }
            Command::HoldDsp => self.dsp_failed = true,
            Command::RestartDsp => {
                self.dsp_failed = false;
                self.dsp_errors = 0;
                let started = self.dsp.start(true);
                self.report_startup();
                match started {
                    Err(e) => self.dsp_error(e),
                    Ok(()) => {
                        if let Some(path) = self.path {
                            self.attach_dsp(path);
                        }
                        for id in 0..MAX_STREAMS {
                            let Some(codec) = self.streams[id].as_ref().and_then(|s| s.codec.dsp_codec()) else { continue };
                            let ok = match self.dsp.codec_open(id, codec) {
                                Ok(()) => true,
                                Err(e) => {
                                    self.dsp_error(e);
                                    false
                                }
                            };
                            if let Some(s) = &mut self.streams[id] {
                                s.dsp_ready = ok;
                            }
                        }
                    }
                }
            }
            Command::Shutdown => {
                for id in 0..MAX_STREAMS {
                    self.stop_stream(id);
                }
                self.tone = None;
                self.end_sound();
                self.ring = None;
                self.close_path();
            }
        }
    }

    fn start_stream(&mut self, id: usize, c: StreamConfig) {
        if id >= MAX_STREAMS {
            self.emit(Event::StreamError { id, error: "no such stream slot".into() });
            return;
        }
        self.stop_stream(id);
        let units = usize::from(c.ptime_ms / 10).clamp(1, MAX_UNITS);
        if let Err(e) = c.rtp.set_nonblocking(true).and(c.rtcp.as_ref().map_or(Ok(()), |s| s.set_nonblocking(true))) {
            self.emit(Event::StreamError { id, error: format!("socket: {e}") });
            return;
        }
        let mut dsp_ready = true;
        if let Some(codec) = c.codec.dsp_codec() {
            if self.dsp_failed {
                dsp_ready = false;
            } else if let Err(e) = self.dsp.codec_open(id, codec) {
                if !e.is_fatal() {
                    self.emit(Event::StreamError { id, error: format!("codec: {e}") });
                    return;
                }
                self.dsp_error(e);
                dsp_ready = false;
            }
        }
        let law = match c.codec {
            PayloadCodec::Pcmu => Some(Law::Mu),
            PayloadCodec::Pcma => Some(Law::A),
            _ => None,
        };
        let (srtp_tx, srtp_rx) = match &c.srtp {
            Some(s) => (Some(SrtpContext::new(s.suite, &s.tx)), Some(SrtpContext::new(s.suite, &s.rx))),
            None => (None, None),
        };
        self.streams[id] = Some(Box::new(Stream {
            codec: c.codec,
            law,
            payload_type: c.payload_type & 0x7f,
            dtmf_pt: c.dtmf_payload_type,
            units_per_packet: units,
            rtp: c.rtp,
            rtcp: c.rtcp,
            remote_rtp: c.remote_rtp,
            remote_rtcp: c.remote_rtcp,
            symmetric: c.symmetric,
            send: c.send,
            recv: c.recv,
            cname: c.cname,
            ssrc: random_u32(),
            seq: random_u32() as u16,
            ts: random_u32(),
            marker: true,
            tx: [0; rtp::RTP_HEADER + MAX_UNITS * UNIT_MAX + SRTP_MAX_OVERHEAD],
            tx_units: 0,
            packets_sent: 0,
            octets_sent: 0,
            sent_since_report: false,
            jitter: JitterBuffer::new(c.codec.unit_bytes(), c.jitter_min_ms, c.jitter_max_ms),
            rx_stats: RxStats::default(),
            rejected: 0,
            last_rx_tick: self.tick,
            timeout_ticks: c.rtp_timeout_ms / 10,
            timed_out: false,
            rx_bands: BandFrame::ZERO,
            plc_last: [0; NB],
            plc_run: 0,
            dtmf_rx: DtmfReceiver::default(),
            dtmf_tx: None,
            dtmf_queue: VecDeque::with_capacity(32),
            dtmf_gap: 0,
            inband: None,
            srtp_tx,
            srtp_rx,
            dsp_ready,
        }));
    }

    fn stop_stream(&mut self, id: usize) {
        let Some(mut s) = self.streams.get_mut(id).and_then(Option::take) else { return };
        self.send_rtcp(&mut s, true);
        drop(s);
        self.conference &= !(1 << id);
    }

    /// One 10 ms tick.  Returns whether the audio device paced it; if not,
    /// the caller waits out the rest of the 10 ms.
    pub fn tick(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        let n = (self.rate / 100) as usize;
        let mut paced = false;

        // 1. microphone
        if self.device_open {
            match self.audio.read(&mut self.mic_pcm[..n]) {
                Ok(()) => {
                    paced = true;
                    self.audio_errors = 0;
                }
                Err(e) => {
                    self.mic_pcm[..n].fill(0);
                    self.audio_errors += 1;
                    if self.audio_errors == 1 {
                        self.emit(Event::AudioError { kind: e.kind() });
                    }
                    // try to get the device back, once a second
                    if self.audio_errors.is_multiple_of(100)
                        && let Some(path) = self.path {
                            self.audio.close();
                            self.device_open = self.audio.open(path).is_ok();
                        }
                }
            }
        } else if self.path.is_some() && self.tick.is_multiple_of(100) {
            // the device failed to open: keep trying
            if let Some(path) = self.path {
                self.device_open = self.audio.open(path).is_ok();
            }
        }

        // 2. and 3. network in, decode
        self.far.clear();
        for id in 0..MAX_STREAMS {
            let Some(mut s) = self.streams[id].take() else { continue };
            self.receive(id, &mut s);
            self.decode_tick(id, &mut s);
            if s.recv {
                self.far.mix(&s.rx_bands);
            }
            self.streams[id] = Some(s);
        }

        // 4. local sounds, then out to the device
        self.update_tone_mode();
        self.local_sounds();
        let dsp_ok = !self.dsp_failed;
        if let (Some(path), true) = (self.path, self.device_open) {
            let mut played = false;
            if dsp_ok {
                let r = self.dsp.egress(path, &mut self.far).and_then(|()| self.dsp.bands_to_speaker(path, &self.far, &mut self.spk_pcm[..n]));
                match r {
                    Ok(()) => played = true,
                    Err(e) => self.dsp_error(e),
                }
            }
            if !played {
                self.spk_pcm[..n].fill(0);
            }
            if let Err(e) = self.audio.write(&self.spk_pcm[..n]) {
                self.audio_errors += 1;
                if self.audio_errors == 1 {
                    self.emit(Event::AudioError { kind: e.kind() });
                }
            }

            // 5. microphone side
            self.mic.clear();
            if dsp_ok && paced {
                let r = self
                    .dsp
                    .mic_to_bands(path, &self.mic_pcm[..n], &mut self.mic)
                    .and_then(|()| self.dsp.ingress(path, &mut self.mic))
                    .and_then(|()| self.dsp.poll(path));
                if let Err(e) = r {
                    self.mic.clear();
                    self.dsp_error(e);
                }
            }
            self.meter(n, true);
        } else {
            self.mic.clear();
            self.meter(n, false);
        }
        if self.mute {
            self.mic.clear();
        }

        // 6. encode and send
        for id in 0..MAX_STREAMS {
            let Some(mut s) = self.streams[id].take() else { continue };
            self.scratch.clone_from(&self.mic);
            if self.conference & (1 << id) != 0 {
                for other in 0..MAX_STREAMS {
                    if other != id && self.conference & (1 << other) != 0
                        && let Some(o) = &self.streams[other]
                            && o.recv {
                                self.scratch.mix(&o.rx_bands);
                            }
                }
            }
            self.encode_tick(id, &mut s);
            self.housekeeping(id, &mut s);
            self.streams[id] = Some(s);
        }
        paced
    }

    /// Reports a sound file that is being stopped or replaced.
    fn end_sound(&mut self) {
        if let Some((token, position, _)) = self.ring.as_ref().and_then(RingPlayer::one_shot) {
            self.ring = None;
            self.emit(Event::SoundEnded { token, position });
        }
    }

    /// Tones and ringing into the far-end mix.
    fn local_sounds(&mut self) {
        if let Some(t) = &mut self.tone
            && !t.mix_into(&mut self.far.nb) {
                self.tone = None;
                self.emit(Event::ToneEnded);
            }
        if let Some((t, left)) = &mut self.feedback {
            t.mix_into(&mut self.far.nb);
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.feedback = None;
            }
        }
        if let Some(ring) = &mut self.ring {
            let mut wb = [0i16; WB];
            ring.fill(&mut wb);
            // The ring is wideband; it goes through a free stream's filter
            // bank (as the stock firmware's stream 0 does for its sounds).
            // With every stream busy, only its lower band is played.
            let slot = (0..MAX_STREAMS).rev().find(|&i| self.streams[i].is_none());
            let mut done = false;
            if let (Some(slot), false) = (slot, self.dsp_failed) {
                match self.dsp.wb_to_bands(slot, &wb, &mut self.scratch) {
                    Ok(()) => {
                        self.far.mix(&self.scratch);
                        done = true;
                    }
                    Err(e) => self.dsp_error(e),
                }
            }
            if !done {
                let mut low = [0i16; NB];
                self.ring_split.low_only(&wb, &mut low);
                mix_into(&mut self.far.nb, &low);
            }
        }
        // A sound file that has played out.
        if self.ring.as_ref().and_then(RingPlayer::one_shot).is_some_and(|(_, _, ended)| ended) {
            self.end_sound();
        }
    }

    fn receive(&mut self, id: usize, s: &mut Stream) {
        let now_ts = self.tick.wrapping_mul(TS_PER_TICK);
        for _ in 0..32 {
            let (len, from) = match s.rtp.recv_from(&mut self.pkt) {
                Ok(v) => v,
                Err(_) => break,
            };
            if !s.recv {
                continue;
            }
            let len = match &mut s.srtp_rx {
                Some(ctx) => match ctx.unprotect(&mut self.pkt[..len]) {
                    Ok(n) => n,
                    Err(_) => {
                        s.rejected += 1;
                        continue;
                    }
                },
                None => len,
            };
            let Some((h, payload)) = rtp::parse(&self.pkt[..len]) else {
                s.rejected += 1;
                continue;
            };
            if s.symmetric && from != s.remote_rtp {
                s.remote_rtp = from;
            }
            s.last_rx_tick = self.tick;
            s.timed_out = false;
            if Some(h.pt) == s.dtmf_pt {
                if let Some(p) = EventPayload::parse(payload) {
                    for edge in s.dtmf_rx.packet(h.ts, &p).into_iter().flatten() {
                        (self.events)(Event::Dtmf { id, edge });
                    }
                }
            } else if h.pt == s.payload_type {
                s.rx_stats.update(&h, now_ts);
                s.jitter.set_jitter(s.rx_stats.jitter());
                s.jitter.insert(h.ts, payload);
            }
            // anything else (comfort noise, unknown types) is dropped
        }
        if let Some(edge) = s.dtmf_rx.tick() {
            (self.events)(Event::Dtmf { id, edge });
        }
        if let Some(rtcp) = &s.rtcp {
            for _ in 0..4 {
                let Ok((len, _)) = rtcp.recv_from(&mut self.pkt) else { break };
                let len = match &mut s.srtp_rx {
                    Some(ctx) => match ctx.unprotect_rtcp(&mut self.pkt[..len]) {
                        Ok(n) => n,
                        Err(_) => continue,
                    },
                    None => len,
                };
                let stats = &mut s.rx_stats;
                rtp::parse_rtcp(&self.pkt[..len], |item| {
                    if let RtcpItem::SenderReport { info, .. } = item {
                        stats.last_sr_ntp_mid = (info.ntp >> 16) as u32;
                        stats.last_sr_at = now_ts;
                    }
                });
            }
        }
    }

    /// Takes 10 ms from the stream's jitter buffer into `s.rx_bands`.
    fn decode_tick(&mut self, id: usize, s: &mut Stream) {
        s.rx_bands.clear();
        if !s.recv {
            return;
        }
        let mut unit = [0u8; UNIT_MAX];
        let pop = s.jitter.pop(&mut unit);
        match (s.law, pop) {
            (_, Pop::Idle) => s.plc_run = 0,
            (Some(law), Pop::Frame) => {
                law.decode(&unit, &mut s.rx_bands.nb);
                s.plc_last = s.rx_bands.nb;
                s.plc_run = 0;
            }
            (Some(_), Pop::Lost) => {
                // repeat the last 10 ms, fading out over three losses
                s.plc_run += 1;
                if s.plc_run <= 3 {
                    s.rx_bands.nb = s.plc_last;
                    scale(&mut s.rx_bands.nb, 4096 >> s.plc_run);
                }
            }
            (None, pop) => {
                if !s.dsp_ready || self.dsp_failed {
                    return;
                }
                let lost = pop == Pop::Lost;
                let r = match s.codec {
                    PayloadCodec::G722 => {
                        let mut wb = [0i16; WB];
                        let r = if lost { self.dsp.decode_lost(id, &mut wb) } else { self.dsp.decode(id, &unit[..80], &mut wb) };
                        r.and_then(|_| self.dsp.wb_to_bands(id, &wb, &mut s.rx_bands))
                    }
                    _ => {
                        // G.726-32: two 5 ms frames of 20 bytes
                        let mut r = Ok(0);
                        for half in 0..2 {
                            let out = &mut s.rx_bands.nb[40 * half..40 * half + 40];
                            r = if lost { self.dsp.decode_lost(id, out) } else { self.dsp.decode(id, &unit[20 * half..20 * half + 20], out) };
                            if r.is_err() {
                                break;
                            }
                        }
                        r.map(drop)
                    }
                };
                if let Err(e) = r {
                    s.rx_bands.clear();
                    self.dsp_error(e);
                }
            }
        }
    }

    /// Encodes `self.scratch` (10 ms for this stream) and sends a packet
    /// when one is full.
    fn encode_tick(&mut self, id: usize, s: &mut Stream) {
        s.ts = s.ts.wrapping_add(TS_PER_TICK);
        if !s.send {
            return;
        }
        // DTMF as tones in the audio
        if let Some((tone, left)) = &mut s.inband {
            self.scratch.clear();
            tone.mix_into(&mut self.scratch.nb);
            *left -= 1;
            if *left == 0 {
                s.inband = None;
                s.dtmf_gap = 5;
            }
        }
        let ub = s.codec.unit_bytes();
        let off = rtp::RTP_HEADER + s.tx_units * ub;
        let out = &mut s.tx[off..off + ub];
        match s.law {
            Some(law) => law.encode(&self.scratch.nb, out),
            None if s.dsp_ready && !self.dsp_failed => {
                let r = match s.codec {
                    PayloadCodec::G722 => {
                        let mut wb = [0i16; WB];
                        self.dsp.bands_to_wb(id, &self.scratch, &mut wb).and_then(|()| self.dsp.encode(id, &wb, out))
                    }
                    _ => self.dsp.encode(id, &self.scratch.nb[..40], &mut out[..20]).and_then(|_| self.dsp.encode(id, &self.scratch.nb[40..], &mut out[20..])),
                };
                if let Err(e) = r {
                    self.dsp_error(e);
                    // nothing sensible to send
                    s.tx_units = 0;
                    return;
                }
            }
            None => {
                s.tx_units = 0;
                return;
            }
        }
        s.tx_units += 1;
        if s.tx_units < s.units_per_packet {
            return;
        }
        let payload_len = s.tx_units * ub;
        s.tx_units = 0;
        let interval = (s.units_per_packet as u32) * TS_PER_TICK;

        // a digit to send as events?
        if s.dtmf_tx.is_none() && s.inband.is_none() {
            if s.dtmf_gap > 0 {
                s.dtmf_gap = s.dtmf_gap.saturating_sub(s.units_per_packet as u32);
            } else if let Some((key, ms, mode)) = s.dtmf_queue.pop_front() {
                if s.dtmf_pt.is_some() && mode == DtmfMode::Auto {
                    s.dtmf_tx = DtmfSender::new(key, s.ts.wrapping_sub(interval), ms);
                } else if let Some(spec) = ToneSpec::dtmf(key, -8) {
                    s.inband = Some((ToneGen::new(spec), ms.clamp(40, 8000) / 10));
                }
            }
        }
        if let (Some(tx), Some(pt)) = (&mut s.dtmf_tx, s.dtmf_pt) {
            match tx.next(interval) {
                Some(ev) => {
                    let h = RtpHeader { marker: ev.marker, pt, seq: s.seq, ts: ev.ts, ssrc: s.ssrc };
                    let mut pkt = [0u8; rtp::RTP_HEADER + 4 + SRTP_MAX_OVERHEAD];
                    rtp::write_header(&mut pkt, &h);
                    pkt[rtp::RTP_HEADER..rtp::RTP_HEADER + 4].copy_from_slice(&ev.payload);
                    Self::send_packet(s, &mut pkt, rtp::RTP_HEADER + 4);
                    return;
                }
                None => {
                    s.dtmf_tx = None;
                    s.dtmf_gap = 4;
                }
            }
        }

        let h = RtpHeader {
            marker: std::mem::take(&mut s.marker),
            pt: s.payload_type,
            seq: s.seq,
            ts: s.ts.wrapping_sub(interval),
            ssrc: s.ssrc,
        };
        let mut tx = s.tx;
        rtp::write_header(&mut tx, &h);
        Self::send_packet(s, &mut tx, rtp::RTP_HEADER + payload_len);
    }

    fn send_packet(s: &mut Stream, buf: &mut [u8], len: usize) {
        s.seq = s.seq.wrapping_add(1);
        let len = match &mut s.srtp_tx {
            Some(ctx) => match ctx.protect(buf, len) {
                Ok(n) => n,
                Err(_) => return,
            },
            None => len,
        };
        // A full socket buffer or a missing route loses the packet, as the
        // network would.
        if s.rtp.send_to(&buf[..len], s.remote_rtp).is_ok() {
            s.packets_sent = s.packets_sent.wrapping_add(1);
            s.octets_sent = s.octets_sent.wrapping_add((len - rtp::RTP_HEADER) as u32);
            s.sent_since_report = true;
        }
    }

    fn send_rtcp(&mut self, s: &mut Stream, bye: bool) {
        let Some(sock) = &s.rtcp else { return };
        let remote = s.remote_rtcp.unwrap_or_else(|| SocketAddr::new(s.remote_rtp.ip(), s.remote_rtp.port().wrapping_add(1)));
        let now_ts = self.tick.wrapping_mul(TS_PER_TICK);
        let block = (s.rx_stats.received > 0).then(|| s.rx_stats.report(now_ts));
        let info = SenderInfo { ntp: rtp::ntp_now(), rtp_ts: s.ts, packets: s.packets_sent, octets: s.octets_sent };
        let sender = std::mem::take(&mut s.sent_since_report).then_some(&info);
        rtp::build_report(&mut self.rtcp_buf, s.ssrc, sender, block.as_ref(), &s.cname, bye);
        let mut len = self.rtcp_buf.len();
        if let Some(ctx) = &mut s.srtp_tx {
            self.rtcp_buf.resize(len + SRTCP_OVERHEAD, 0);
            match ctx.protect_rtcp(&mut self.rtcp_buf, len) {
                Ok(n) => len = n,
                Err(_) => return,
            }
        }
        let _ = sock.send_to(&self.rtcp_buf[..len], remote);
    }

    fn housekeeping(&mut self, id: usize, s: &mut Stream) {
        if s.recv && s.timeout_ticks != 0 && !s.timed_out && self.tick.wrapping_sub(s.last_rx_tick) > s.timeout_ticks {
            s.timed_out = true;
            self.emit(Event::RtpTimeout { id });
        }
        if self.tick % REPORT_TICKS == (id as u32 * 7) % REPORT_TICKS {
            self.send_rtcp(s, false);
            let stats = StreamStats {
                packets_sent: s.packets_sent,
                octets_sent: s.octets_sent,
                packets_received: s.rx_stats.received,
                packets_lost: s.rx_stats.lost(),
                jitter_ms: s.rx_stats.jitter() / 8,
                jitter_buffer: s.jitter.stats(),
                rejected: s.rejected,
            };
            self.emit(Event::Stats { id, stats });
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EngineOptions {
    /// `SCHED_FIFO` priority for the audio thread; `None` leaves the
    /// scheduler alone.  Failing to get it (no privilege) is not an error.
    pub rt_priority: Option<i32>,
    /// Commands that may wait in the channel.
    pub queue: usize,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self { rt_priority: Some(50), queue: 64 }
    }
}

/// The engine on its thread.
pub struct Engine {
    tx: SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
    meters: Arc<Meters>,
}

#[derive(Debug)]
pub enum SendError {
    /// The command queue is full; the command was not sent.
    Full(Command),
    /// The engine thread is gone.
    Stopped,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Full(_) => f.write_str("audio engine command queue is full"),
            SendError::Stopped => f.write_str("audio engine has stopped"),
        }
    }
}

impl std::error::Error for SendError {}

/// Asks for `SCHED_FIFO` at `priority` for the calling thread.  On a refusal
/// (`EPERM` without the privilege) the thread stays as it was and the error
/// number is returned.
fn set_realtime(priority: i32) -> Result<(), i32> {
    // SAFETY: sets the calling thread's scheduling policy; the parameter
    // structure is initialized.
    let rc = unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = priority;
        libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param)
    };
    if rc == 0 { Ok(()) } else { Err(rc) }
}

impl Engine {
    /// Starts the audio thread.  `events` is called on that thread: it must
    /// not block (hand the event to a channel).
    pub fn spawn(
        audio: Box<dyn AudioDevice>,
        dsp: Box<dyn Dsp>,
        events: impl FnMut(Event) + Send + 'static,
        opts: EngineOptions,
    ) -> io::Result<Self> {
        let (tx, rx) = sync_channel(opts.queue.max(1));
        let meters = Arc::new(Meters::default());
        let core_meters = meters.clone();
        let thread = std::thread::Builder::new().name("audio".into()).spawn(move || {
            let mut events = events;
            if let Some(priority) = opts.rt_priority
                && let Err(errno) = set_realtime(priority)
            {
                events(Event::RealtimeRefused { priority, errno });
            }
            let mut core = EngineCore::new(audio, dsp, Box::new(events));
            core.meters = core_meters;
            core.start();
            run(&mut core, &rx);
        })?;
        Ok(Self { tx, thread: Some(thread), meters })
    }

    /// The level meters; see [`Meters`].
    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
    }

    /// Queues a command; never blocks.
    pub fn send(&self, cmd: Command) -> Result<(), SendError> {
        self.tx.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(c) => SendError::Full(c),
            TrySendError::Disconnected(_) => SendError::Stopped,
        })
    }

    /// Stops the thread and waits for it.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(t) = self.thread.take() {
            // blocking send: the shutdown must not be lost to a full queue
            let _ = self.tx.send(Command::Shutdown);
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(core: &mut EngineCore, rx: &Receiver<Command>) {
    const TICK: Duration = Duration::from_millis(10);
    let mut next = Instant::now() + TICK;
    loop {
        loop {
            match rx.try_recv() {
                Ok(Command::Shutdown) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    core.handle(Command::Shutdown);
                    return;
                }
                Ok(cmd) => core.handle(cmd),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        if !core.is_active() {
            // nothing to do: sleep until a command comes
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    core.handle(Command::Shutdown);
                    return;
                }
                Ok(cmd) => core.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            next = Instant::now() + TICK;
            continue;
        }
        if core.tick() {
            // the capture clock paced this tick
            next = Instant::now() + TICK;
        } else {
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
                next += TICK;
            } else if now - next > 10 * TICK {
                next = now + TICK;
            } else {
                next += TICK;
            }
        }
    }
}
