// SPDX-License-Identifier: GPL-2.0-or-later
//! The engine end to end on a workstation: a fake audio device, the real
//! DSP adapter on a mock transport, and real UDP sockets on loopback.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opendeskphone_media::dtmf::{DtmfEdge, EventPayload};
use opendeskphone_media::fake::{FakeAudio, FakeAudioState};
use opendeskphone_media::Law;
use opendeskphone_media::ring::RingSource;
use opendeskphone_media::rtp::{self, RtpHeader};
use opendeskphone_media::srtp::{MasterKey, SrtpContext, Suite};
use opendeskphone_media::tone::{Osc, ToneSpec};
use opendeskphone_media::{AudioPath, Command, DtmfMode, Engine, EngineCore, EngineOptions, Event, PayloadCodec, SrtpConfig, StreamConfig, VpmDsp, VpmDspOptions};
use opendeskphone_vpm::transport::{MockTransport, Recorded};
use opendeskphone_vpm::{Family, Vpm};

fn goertzel(x: &[i16], f: u32, rate: u32) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f64::from(f) / f64::from(rate);
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &v in x {
        let s0 = f64::from(v) + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).sqrt() * 2.0 / x.len() as f64
}

struct Rig {
    core: EngineCore,
    audio: Arc<Mutex<FakeAudioState>>,
    events: Receiver<Event>,
    jobs: Arc<Mutex<Vec<Recorded>>>,
    rate: u32,
}

/// An engine for `family` whose microphone hears a sine of `mic_hz`.
fn rig(family: Family, mic_hz: u32) -> Rig {
    let rate = family.device_rate();
    let (audio, state) = FakeAudio::new(rate);
    let mut osc = Osc::new(mic_hz, rate, 8000);
    state.lock().unwrap().mic = Some(Box::new(move |buf| buf.iter_mut().for_each(|s| *s = osc.next())));
    // a mock that also copies its job log where the test can see it
    let jobs = Arc::new(Mutex::new(Vec::new()));
    let log = jobs.clone();
    let transport = MockTransport::with_handler(family, move |req, reply| {
        log.lock().unwrap().push(Recorded {
            channel: req.channel,
            client: req.client,
            instance: req.instance,
            opcode: req.opcode,
            codec: req.codec,
            arg1: req.arg1,
            arg2: req.arg2,
            body: Vec::new(),
        });
        MockTransport::default_answer(family, req, reply)
    });
    let mut transport = transport;
    transport.keep = 1;
    let dsp = VpmDsp::new(Vpm::new(transport).unwrap(), VpmDspOptions::default());
    let (tx, events) = channel();
    let mut core = EngineCore::new(Box::new(audio), Box::new(dsp), Box::new(move |e| drop(tx.send(e))));
    core.start();
    jobs.lock().unwrap().clear();
    Rig { core, audio: state, events, jobs, rate }
}

/// A far end: a socket, and the address the phone's stream is bound to.
fn peer() -> (UdpSocket, UdpSocket, SocketAddr) {
    let far = UdpSocket::bind("127.0.0.1:0").unwrap();
    far.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let local = UdpSocket::bind("127.0.0.1:0").unwrap();
    let local_addr = local.local_addr().unwrap();
    (far, local, local_addr)
}

fn drain(sock: &UdpSocket) -> Vec<Vec<u8>> {
    sock.set_nonblocking(true).unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 1500];
    while let Ok((n, _)) = sock.recv_from(&mut buf) {
        out.push(buf[..n].to_vec());
    }
    sock.set_nonblocking(false).unwrap();
    out
}

fn rtp_packet(pt: u8, seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; 12];
    rtp::write_header(&mut p, &RtpHeader { marker, pt, seq, ts, ssrc: 0x1234 });
    p.extend_from_slice(payload);
    p
}

#[test]
fn g711_call_both_ways_old_family() {
    let mut r = rig(Family::Old, 1000);
    let (far, local, local_addr) = peer();
    r.core.handle(Command::SetPath(Some(AudioPath::Handset)));
    r.core.handle(Command::StartStream { id: 1, config: Box::new(StreamConfig::new(PayloadCodec::Pcmu, 0, local, far.local_addr().unwrap())) });

    // the far end sends a 440 Hz tone, 20 ms per packet
    let mut osc = Osc::new(440, 8000, 6000);
    let mut sent = Vec::new();
    for i in 0..150u32 {
        if i % 2 == 0 {
            let pcm: Vec<i16> = (0..160).map(|_| osc.next()).collect();
            let mut payload = vec![0u8; 160];
            Law::Mu.encode(&pcm, &mut payload);
            far.send_to(&rtp_packet(0, i as u16 / 2, i * 80, i == 0, &payload), local_addr).unwrap();
        }
        assert!(r.core.tick());
        sent.extend(drain(&far));
    }

    // what the phone sent: 20 ms PCMU packets carrying the microphone's tone
    assert!((70..=75).contains(&sent.len()), "{} packets", sent.len());
    let mut audio = Vec::new();
    let mut prev: Option<RtpHeader> = None;
    for p in &sent {
        let (h, payload) = rtp::parse(p).unwrap();
        assert_eq!((h.pt, payload.len()), (0, 160));
        if let Some(q) = prev {
            assert_eq!(h.seq, q.seq.wrapping_add(1));
            assert_eq!(h.ts, q.ts.wrapping_add(160));
            assert_eq!(h.ssrc, q.ssrc);
            assert!(!h.marker);
        } else {
            assert!(h.marker);
        }
        prev = Some(h);
        let mut pcm = vec![0i16; 160];
        Law::Mu.decode(payload, &mut pcm);
        audio.extend(pcm);
    }
    let level = goertzel(&audio[1600..], 1000, 8000);
    assert!((7000.0..9000.0).contains(&level), "microphone tone at the far end: {level}");

    // what the phone played: the far end's tone, at the device rate
    let played = r.audio.lock().unwrap().played.clone();
    assert_eq!(played.len(), 150 * 160);
    let level = goertzel(&played[8000..], 440, r.rate);
    assert!((5000.0..7000.0).contains(&level), "far-end tone at the receiver: {level}");
    assert!(goertzel(&played[8000..], 1000, r.rate) < 100.0, "no sidetone from the engine");

    // old family, G.711 handset call: three DSP jobs per tick (HEC egress, ingress, get-state)
    r.jobs.lock().unwrap().clear();
    r.core.tick();
    let jobs: Vec<_> = r.jobs.lock().unwrap().iter().map(|j| (j.client, j.channel, j.opcode)).collect();
    assert_eq!(jobs, [(4, 6, 3), (4, 6, 2), (4, 6, 8)]);

    // mute: the far end gets silence
    r.core.handle(Command::MuteMic(true));
    drain(&far);
    for _ in 0..10 {
        r.core.tick();
    }
    let muted = drain(&far);
    let (_, payload) = rtp::parse(muted.last().unwrap()).unwrap();
    assert!(payload.iter().all(|&b| b == 0xff));

    // stopping the stream ends the traffic
    r.core.handle(Command::StopStream { id: 1 });
    for _ in 0..4 {
        r.core.tick();
    }
    assert!(drain(&far).is_empty());
}

#[test]
fn g722_call_new_family_sends_the_stock_job_set() {
    let mut r = rig(Family::New, 1000);
    let (far, local, local_addr) = peer();
    r.core.handle(Command::SetPath(Some(AudioPath::Handset)));
    r.jobs.lock().unwrap().clear();
    r.core.handle(Command::StartStream { id: 1, config: Box::new(StreamConfig::new(PayloadCodec::G722, 9, local, far.local_addr().unwrap())) });
    // call setup as captured: split init, decoder init, combine init, encoder init
    let setup: Vec<_> = r.jobs.lock().unwrap().iter().map(|j| (j.client, j.channel, j.opcode, j.codec)).collect();
    assert_eq!(setup, [(8, 6, 0, 0), (1, 5, 3, 46), (8, 7, 3, 0), (1, 4, 0, 46)]);

    // the far end loops the phone's packets back
    let mut packets = 0;
    for _ in 0..200 {
        r.core.tick();
        for p in drain(&far) {
            let (h, payload) = rtp::parse(&p).unwrap();
            assert_eq!((h.pt, payload.len()), (9, 160));
            packets += 1;
            far.send_to(&p, local_addr).unwrap();
        }
    }
    assert!((95..=100).contains(&packets), "{packets}");

    // steady state, per 10 ms: the nine jobs of a stock G.722 handset call
    r.jobs.lock().unwrap().clear();
    r.core.tick();
    let mut jobs: Vec<_> = r.jobs.lock().unwrap().iter().map(|j| (j.client, j.channel, j.opcode)).collect();
    let order = jobs.clone();
    jobs.sort();
    assert_eq!(
        jobs,
        [
            (1, 4, 2),  // encode
            (1, 5, 5),  // decode
            (4, 12, 2), // HEC ingress
            (4, 12, 3), // HEC egress
            (4, 12, 8), // HEC get-state
            (8, 6, 2),  // stream split wideband
            (8, 7, 5),  // stream combine wideband
            (8, 13, 1), // device split
            (8, 14, 4), // device combine
        ]
    );
    // decode before the canceller's egress, ingress before encode
    let pos = |j| order.iter().position(|&x| x == j).unwrap();
    assert!(pos((1, 5, 5)) < pos((4, 12, 3)));
    assert!(pos((4, 12, 3)) < pos((4, 12, 2)));
    assert!(pos((4, 12, 2)) < pos((1, 4, 2)));

    // the looped-back microphone tone reaches the receiver
    let played = r.audio.lock().unwrap().played.clone();
    let level = goertzel(&played[32_000..], 1000, r.rate);
    assert!(level > 2000.0, "looped tone at the receiver: {level}");

    // switching to the speakerphone moves the device-side jobs to the SBAEC
    r.core.handle(Command::SetPath(Some(AudioPath::Speaker)));
    r.jobs.lock().unwrap().clear();
    r.core.tick();
    let mut jobs: Vec<_> = r.jobs.lock().unwrap().iter().map(|j| (j.client, j.channel, j.opcode)).filter(|j| j.0 != 1 && j.1 > 7).collect();
    jobs.sort();
    assert_eq!(jobs, [(3, 18, 7), (3, 18, 8), (3, 18, 9), (8, 19, 1), (8, 20, 4)]);
    assert_eq!(r.audio.lock().unwrap().open, Some(AudioPath::Speaker));
}

#[test]
fn g726_call_uses_two_frames_per_tick() {
    for family in [Family::Old, Family::New] {
        let mut r = rig(family, 500);
        let (far, local, local_addr) = peer();
        r.core.handle(Command::SetPath(Some(AudioPath::Speaker)));
        r.core.handle(Command::StartStream { id: 0, config: Box::new(StreamConfig::new(PayloadCodec::G726_32, 98, local, far.local_addr().unwrap())) });
        for _ in 0..60 {
            r.core.tick();
            for p in drain(&far) {
                let (h, payload) = rtp::parse(&p).unwrap();
                assert_eq!((h.pt, payload.len()), (98, 80));
                far.send_to(&p, local_addr).unwrap();
            }
        }
        r.jobs.lock().unwrap().clear();
        r.core.tick();
        let pve: Vec<_> = r.jobs.lock().unwrap().iter().filter(|j| j.client == 1).map(|j| (j.opcode, j.codec)).collect();
        assert_eq!(pve, [(5, 4), (5, 4), (2, 4), (2, 4)], "{family:?}");
        // no stream-side filter bank for a narrowband codec
        assert!(!r.jobs.lock().unwrap().iter().any(|j| j.client == 8 && j.channel < 12));
    }
}

#[test]
fn srtp_both_directions() {
    let mut r = rig(Family::Old, 1000);
    let (far, local, local_addr) = peer();
    let (phone_key, far_key) = (MasterKey::generate().unwrap(), MasterKey::generate().unwrap());
    let mut config = StreamConfig::new(PayloadCodec::Pcma, 8, local, far.local_addr().unwrap());
    config.srtp = Some(SrtpConfig { suite: Suite::AesCm128HmacSha1_80, tx: phone_key.clone(), rx: far_key.clone() });
    r.core.handle(Command::SetPath(Some(AudioPath::Handset)));
    r.core.handle(Command::StartStream { id: 0, config: Box::new(config) });
    let mut far_rx = SrtpContext::new(Suite::AesCm128HmacSha1_80, &phone_key);
    let mut far_tx = SrtpContext::new(Suite::AesCm128HmacSha1_80, &far_key);

    let mut osc = Osc::new(700, 8000, 5000);
    let mut got = Vec::new();
    for i in 0..100u32 {
        if i % 2 == 0 {
            let pcm: Vec<i16> = (0..160).map(|_| osc.next()).collect();
            let mut payload = vec![0u8; 160];
            Law::A.encode(&pcm, &mut payload);
            let mut p = rtp_packet(8, i as u16, i * 80, false, &payload);
            let len = p.len();
            p.resize(len + 10, 0);
            let n = far_tx.protect(&mut p, len).unwrap();
            far.send_to(&p[..n], local_addr).unwrap();
            // a forged packet is ignored
            far.send_to(&rtp_packet(8, 9000 + i as u16, i * 80, false, &[0x55; 170]), local_addr).unwrap();
        }
        r.core.tick();
        for mut p in drain(&far) {
            assert_eq!(p.len(), 12 + 160 + 10);
            let n = far_rx.unprotect(&mut p).expect("the phone's packets verify");
            let (_, payload) = rtp::parse(&p[..n]).unwrap();
            let mut pcm = vec![0i16; 160];
            Law::A.decode(payload, &mut pcm);
            got.extend(pcm);
        }
    }
    assert!(goertzel(&got[1600..], 1000, 8000) > 7000.0);
    let played = r.audio.lock().unwrap().played.clone();
    assert!(goertzel(&played[6400..], 700, r.rate) > 4000.0);
}

#[test]
fn dtmf_events_and_inband() {
    let mut r = rig(Family::Old, 0);
    let (far, local, local_addr) = peer();
    let mut config = StreamConfig::new(PayloadCodec::Pcmu, 0, local, far.local_addr().unwrap());
    config.dtmf_payload_type = Some(101);
    r.core.handle(Command::StartStream { id: 2, config: Box::new(config) });
    for _ in 0..10 {
        r.core.tick();
    }
    drain(&far);

    // sending: event packets replace the audio
    r.core.handle(Command::SendDtmf { id: 2, key: '7', duration_ms: 100, mode: DtmfMode::Auto, feedback: false });
    let mut events = Vec::new();
    let mut audio_after = 0;
    for _ in 0..40 {
        r.core.tick();
        for p in drain(&far) {
            let (h, payload) = rtp::parse(&p).unwrap();
            if h.pt == 101 {
                events.push((h, EventPayload::parse(payload).unwrap()));
            } else if !events.is_empty() {
                audio_after += 1;
            }
        }
    }
    assert_eq!(events.len(), 7);
    assert!(events[0].0.marker);
    assert!(events.iter().all(|(h, p)| h.ts == events[0].0.ts && p.event == 7));
    assert!(events.windows(2).all(|w| w[1].0.seq == w[0].0.seq.wrapping_add(1)));
    assert_eq!(events.iter().filter(|(_, p)| p.end).count(), 3);
    assert_eq!(events.last().unwrap().1.duration, 800);
    assert!(audio_after > 5, "audio resumes after the digit");

    // in-band: the tone pair is in the audio
    r.core.handle(Command::SendDtmf { id: 2, key: '7', duration_ms: 200, mode: DtmfMode::Inband, feedback: false });
    let mut audio = Vec::new();
    for _ in 0..40 {
        r.core.tick();
        for p in drain(&far) {
            let (h, payload) = rtp::parse(&p).unwrap();
            assert_eq!(h.pt, 0);
            let mut pcm = vec![0i16; payload.len()];
            Law::Mu.decode(payload, &mut pcm);
            audio.extend(pcm);
        }
    }
    assert!(goertzel(&audio, 852, 8000) > 800.0);
    assert!(goertzel(&audio, 1209, 8000) > 800.0);
    assert!(goertzel(&audio, 697, 8000) < 100.0);

    // receiving: one start and one end per digit
    while r.events.try_recv().is_ok() {}
    for (i, (end, dur)) in [(false, 160), (false, 320), (true, 480), (true, 480), (true, 480)].into_iter().enumerate() {
        let mut payload = [0u8; 4];
        EventPayload { event: 3, end, volume: 10, duration: dur }.write(&mut payload);
        far.send_to(&rtp_packet(101, 50 + i as u16, 4000, i == 0, &payload), local_addr).unwrap();
        r.core.tick();
        r.core.tick();
    }
    let got: Vec<_> = r.events.try_iter().filter(|e| matches!(e, Event::Dtmf { .. })).collect();
    assert_eq!(got, [Event::Dtmf { id: 2, edge: DtmfEdge::Start('3') }, Event::Dtmf { id: 2, edge: DtmfEdge::End('3', 60) }]);
}

#[test]
fn conference_bridges_two_streams() {
    let mut r = rig(Family::Old, 1000);
    let (far_a, local_a, addr_a) = peer();
    let (far_b, local_b, _) = peer();
    r.core.handle(Command::SetPath(Some(AudioPath::Speaker)));
    r.core.handle(Command::StartStream { id: 0, config: Box::new(StreamConfig::new(PayloadCodec::Pcmu, 0, local_a, far_a.local_addr().unwrap())) });
    r.core.handle(Command::StartStream { id: 1, config: Box::new(StreamConfig::new(PayloadCodec::Pcma, 8, local_b, far_b.local_addr().unwrap())) });
    let mut osc = Osc::new(1500, 8000, 6000);
    let mut b_hears = Vec::new();
    for phase in 0..2 {
        if phase == 1 {
            r.core.handle(Command::SetConference { mask: 0b011 });
        }
        b_hears.clear();
        for i in 0..100u32 {
            let pcm: Vec<i16> = (0..80).map(|_| osc.next()).collect();
            let mut payload = vec![0u8; 80];
            Law::Mu.encode(&pcm, &mut payload);
            far_a.send_to(&rtp_packet(0, (phase * 100 + i) as u16, (phase * 100 + i) * 80, false, &payload), addr_a).unwrap();
            r.core.tick();
            for p in drain(&far_b) {
                let (_, payload) = rtp::parse(&p).unwrap();
                let mut pcm = vec![0i16; payload.len()];
                Law::A.decode(payload, &mut pcm);
                b_hears.extend(pcm);
            }
        }
        let from_a = goertzel(&b_hears[1600..], 1500, 8000);
        let from_mic = goertzel(&b_hears[1600..], 1000, 8000);
        assert!(from_mic > 7000.0, "B always hears the phone's user: {from_mic}");
        if phase == 0 {
            assert!(from_a < 100.0, "not bridged yet: {from_a}");
        } else {
            assert!(from_a > 5000.0, "bridged: {from_a}");
        }
    }
    // hold on A: nothing is sent to it and its audio is not played
    r.core.handle(Command::SetDirection { id: 0, send: false, recv: false });
    drain(&far_a);
    for _ in 0..10 {
        r.core.tick();
    }
    assert!(drain(&far_a).is_empty());
}

#[test]
fn tones_ring_and_volume() {
    for family in [Family::Old, Family::New] {
        let mut r = rig(family, 0);
        r.core.handle(Command::SetPath(Some(AudioPath::Speaker)));
        r.jobs.lock().unwrap().clear();
        r.core.handle(Command::PlayTone(Some(ToneSpec::DIAL)));
        for _ in 0..100 {
            r.core.tick();
        }
        // the canceller is told about the tone, as the stock firmware
        // does: option bit 1, and bit 15 of the second tone register
        let tone_reg = if family == Family::New { 330 } else { 329 };
        let sets = |r: &Rig| -> Vec<(u16, u16)> {
            r.jobs.lock().unwrap().iter().filter(|j| (j.client, j.opcode) == (3, 4)).map(|j| (j.arg1, j.arg2)).collect()
        };
        let on = sets(&r);
        assert_eq!(on.len(), 2, "{family:?}: {on:?}");
        assert_eq!(on[0], (0, 2), "{family:?}");
        assert_eq!((on[1].0, on[1].1 & 0x8000), (tone_reg, 0x8000), "{family:?}");
        // its frame jobs pass 80 samples, and the mock (which answers as
        // a D60's DSP does) gave back whole frames
        assert!(r.jobs.lock().unwrap().iter().filter(|j| j.client == 3 && matches!(j.opcode, 8 | 9)).all(|j| j.arg1 == 80), "{family:?}");
        assert_eq!(r.core.meters().dsp_errors.load(std::sync::atomic::Ordering::Relaxed), 0, "{family:?}");
        r.jobs.lock().unwrap().clear();
        r.core.handle(Command::PlayTone(None));
        r.core.tick();
        let off = sets(&r);
        assert_eq!(off.len(), 2, "{family:?}: {off:?}");
        assert_eq!((off[0], off[1].0, off[1].1 & 0x8000), ((0, 0), tone_reg, 0), "{family:?}");
        r.core.handle(Command::PlayTone(Some(ToneSpec::DIAL)));
        r.audio.lock().unwrap().played.clear();
        for _ in 0..100 {
            r.core.tick();
        }
        let played = std::mem::take(&mut r.audio.lock().unwrap().played);
        assert!(goertzel(&played[played.len() / 2..], 350, r.rate) > 3000.0, "{family:?}");
        assert!(goertzel(&played[played.len() / 2..], 440, r.rate) > 3000.0, "{family:?}");

        r.core.handle(Command::PlayTone(Some(ToneSpec::BEEP)));
        for _ in 0..30 {
            r.core.tick();
        }
        assert!(r.events.try_iter().any(|e| e == Event::ToneEnded));
        r.audio.lock().unwrap().played.clear();

        let ring: std::sync::Arc<[i16]> =
            (0..4800).map(|i| ((i as f64 * 988.0 * std::f64::consts::TAU / 16_000.0).sin() * 8000.0) as i16).collect();
        r.core.handle(Command::PlayRing(Some(RingSource::Pcm { samples: ring, pause_ms: 2000 }), 4096));
        for _ in 0..30 {
            r.core.tick();
        }
        let played = std::mem::take(&mut r.audio.lock().unwrap().played);
        assert!(goertzel(&played[..played.len() / 2], 988, r.rate) > 1000.0, "{family:?}");
        r.core.handle(Command::PlayRing(None, 4096));
        assert!(!r.events.try_iter().any(|e| matches!(e, Event::SoundEnded { .. })), "a ring is not a sound file");

        // The gain of a sounding ring changes without starting it again
        // (the volume keys while the ringer sounds): three passes of a
        // 0.1 s burst and a 0.1 s pause, the middle one at no gain.
        let burst: std::sync::Arc<[i16]> =
            (0..1600).map(|i| ((i as f64 * 1000.0 * std::f64::consts::TAU / 16_000.0).sin() * 8000.0) as i16).collect();
        r.core.handle(Command::SetRingGain(0));
        r.core.handle(Command::PlayRing(Some(RingSource::Pcm { samples: burst, pause_ms: 100 }), 4096));
        let mut passes = Vec::new();
        for gain in [4096, 0, 4096] {
            r.core.handle(Command::SetRingGain(gain));
            r.audio.lock().unwrap().played.clear();
            for _ in 0..20 {
                r.core.tick();
            }
            let played = std::mem::take(&mut r.audio.lock().unwrap().played);
            let (burst, pause) = played.split_at(played.len() / 2);
            passes.push((goertzel(burst, 1000, r.rate), goertzel(pause, 1000, r.rate)));
        }
        assert!(passes[0].0 > 1000.0 && passes[2].0 > 1000.0, "{family:?}: {passes:?}");
        assert!(passes[1].0 < 50.0, "{family:?}: {passes:?}");
        // Had a gain change started the ring again, a burst would have
        // sounded where a pause belongs.
        assert!(passes.iter().all(|p| p.1 < p.0.max(1000.0) / 4.0), "{family:?}: {passes:?}");
        r.core.handle(Command::PlayRing(None, 4096));
        r.audio.lock().unwrap().played.clear();

        // A sound file: 0.25 s of 1 kHz played once from 0.05 s in; the
        // engine says where it ended.
        let file: std::sync::Arc<[i16]> =
            (0..4000).map(|i| ((i as f64 * 1000.0 * std::f64::consts::TAU / 16_000.0).sin() * 8000.0) as i16).collect();
        r.audio.lock().unwrap().played.clear();
        r.core.handle(Command::PlayRing(Some(RingSource::Once { samples: file.clone(), start: 800, token: 5 }), 4096));
        for _ in 0..10 {
            r.core.tick();
        }
        assert!(!r.events.try_iter().any(|e| matches!(e, Event::SoundEnded { .. })), "{family:?}: still playing");
        for _ in 0..20 {
            r.core.tick();
        }
        assert!(r.events.try_iter().any(|e| e == Event::SoundEnded { token: 5, position: 4000 }), "{family:?}");
        let played = std::mem::take(&mut r.audio.lock().unwrap().played);
        assert!(goertzel(&played[..played.len() / 2], 1000, r.rate) > 1000.0, "{family:?}");
        // Stopped, and replaced by another, part way through.
        r.core.handle(Command::PlayRing(Some(RingSource::Once { samples: file.clone(), start: 0, token: 6 }), 4096));
        for _ in 0..5 {
            r.core.tick();
        }
        r.core.handle(Command::PlayRing(Some(RingSource::Once { samples: file, start: 0, token: 7 }), 4096));
        r.core.tick();
        r.core.handle(Command::PlayRing(None, 4096));
        let ends: Vec<Event> = r.events.try_iter().filter(|e| matches!(e, Event::SoundEnded { .. })).collect();
        assert_eq!(
            ends,
            [Event::SoundEnded { token: 6, position: 800 }, Event::SoundEnded { token: 7, position: 160 }],
            "{family:?}"
        );

        // volume goes to the canceller's gain register (SBAEC register 2)
        r.jobs.lock().unwrap().clear();
        r.core.handle(Command::SetRxGain { path: AudioPath::Speaker, gain_q8: -6 * 256 });
        let j = r.jobs.lock().unwrap().clone();
        assert_eq!(j.len(), 1);
        assert_eq!((j[0].client, j[0].opcode, j[0].arg1, j[0].arg2), (3, 4, 2, 0xfa00));

        // closing the path stops the device
        r.core.handle(Command::SetPath(None));
        assert_eq!(r.audio.lock().unwrap().open, None);
        assert!(!r.core.is_active());
        assert!(!r.core.tick());
    }
}

#[test]
fn rtp_timeout_and_stats() {
    let mut r = rig(Family::Old, 0);
    let (far, local, local_addr) = peer();
    let mut config = StreamConfig::new(PayloadCodec::Pcmu, 0, local, far.local_addr().unwrap());
    config.rtp_timeout_ms = 500;
    r.core.handle(Command::StartStream { id: 0, config: Box::new(config) });
    for i in 0..20u32 {
        far.send_to(&rtp_packet(0, i as u16, i * 160, false, &[0xff; 160]), local_addr).unwrap();
        r.core.tick();
        r.core.tick();
    }
    assert!(!r.events.try_iter().any(|e| matches!(e, Event::RtpTimeout { .. })));
    for _ in 0..520 {
        r.core.tick();
    }
    let events: Vec<_> = r.events.try_iter().collect();
    assert_eq!(events.iter().filter(|e| matches!(e, Event::RtpTimeout { id: 0 })).count(), 1);
    let stats = events
        .iter()
        .rev()
        .find_map(|e| if let Event::Stats { stats, .. } = e { Some(*stats) } else { None })
        .expect("a statistics event every five seconds");
    assert_eq!(stats.packets_received, 20);
    assert_eq!(stats.packets_lost, 0);
    assert!(stats.packets_sent >= 240);
}

#[test]
fn dsp_failure_is_reported_once_and_recovers() {
    let rate = Family::New.device_rate();
    let (audio, _state) = FakeAudio::new(rate);
    let fail = Arc::new(Mutex::new(false));
    let f = fail.clone();
    let transport = MockTransport::with_handler(Family::New, move |req, reply| {
        if *f.lock().unwrap() {
            return Err(opendeskphone_vpm::Error::Failed);
        }
        MockTransport::default_answer(Family::New, req, reply)
    });
    let dsp = VpmDsp::new(Vpm::new(transport).unwrap(), VpmDspOptions::default());
    let (tx, events) = channel();
    let mut core = EngineCore::new(Box::new(audio), Box::new(dsp), Box::new(move |e| drop(tx.send(e))));
    core.start();
    core.handle(Command::SetPath(Some(AudioPath::Handset)));
    for _ in 0..5 {
        core.tick();
    }
    *fail.lock().unwrap() = true;
    for _ in 0..50 {
        core.tick();
    }
    let got: Vec<_> = events.try_iter().filter(|e| !matches!(e, Event::DspStartup { .. })).collect();
    assert_eq!(got, [Event::DspError { error: opendeskphone_vpm::Error::Failed, needs_restart: true }]);
    *fail.lock().unwrap() = false;
    core.handle(Command::RestartDsp);
    for _ in 0..5 {
        core.tick();
    }
    assert!(events.try_iter().all(|e| matches!(e, Event::DspStartup { .. })));
}

#[test]
fn engine_thread_runs_and_stops() {
    let (audio, state) = FakeAudio::new(16_000);
    state.lock().unwrap().read_delay = Some(Duration::from_millis(2));
    let dsp = VpmDsp::new(Vpm::new(MockTransport::new(Family::Old)).unwrap(), VpmDspOptions::default());
    let (tx, events) = channel();
    let engine = Engine::spawn(Box::new(audio), Box::new(dsp), move |e| drop(tx.send(e)), EngineOptions { rt_priority: None, queue: 8 }).unwrap();
    engine.send(Command::SetPath(Some(AudioPath::Speaker))).unwrap();
    engine.send(Command::PlayTone(Some(ToneSpec::BEEP))).unwrap();
    let ended = loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::DspStartup { .. } => {}
            other => break other,
        }
    };
    assert_eq!(ended, Event::ToneEnded);
    engine.shutdown();
    let s = state.lock().unwrap();
    assert_eq!(s.open, None);
    assert!(s.played.len() >= 12 * 160);
}
