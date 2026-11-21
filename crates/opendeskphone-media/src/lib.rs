// SPDX-License-Identifier: GPL-2.0-or-later
//! The media path of Open Desk Phone: everything between the network and the
//! audio front end that the ARM does.
//!
//! The DSP runs the codecs other than G.711, the echo cancellers, and (on the
//! new family) the band filter, as jobs through `opendeskphone-vpm`.  (The
//! D80 has no such DSP: see [`softdsp`].)  The rest is here: RTP/RTCP, SRTP, the jitter buffer, RFC 4733 DTMF, G.711 (coded by the `audio-codec` crate), tones and
//! ringing, and the real-time engine that ties them to the audio device and
//! the DSP.
//!
//! # What has run where
//!
//! On a workstation: unit tests (RFC 3711 vectors, G.711, the filter bank,
//! tones, the jitter buffer) and the engine end to end against a fake audio
//! device, loopback sockets, and the DSP adapter on a mock transport.  On a
//! D60: local tones on the handset, headset, and speaker paths with the
//! cancellers running.  No call has run on a phone.
//!
//! # Experiments to run on a phone
//!
//! The routing around the canceller and filter-bank jobs is partly read off
//! traces and partly inferred ([`vpmdsp`] lists which is which).  Each
//! inferred part has an experiment that settles it, using
//! `opendeskphone_vpm::diag` from the `dphone` diagnostics subcommands.  Run
//! the first three with no audio path open, so that the engine sends no jobs
//! of its own in between:
//!
//! 1. **Buffer layout of the filter bank** (`dphone dsp bands 2 --sweep` and
//!    `dphone dsp bands 2 --tone 12000`; `diag::band_experiment`): split and
//!    combine a sweep and a 12 kHz tone.  Expect the tone only in the
//!    third buffer, the combine result's third buffer equal to the input
//!    delayed, and its second buffer the 16 kHz signal.
//! 2. **Buffers through the cancellers** (`dphone dsp canceller
//!    hec0|hec1|sbaec ingress|egress BUFFER HZ`;
//!    `diag::canceller_experiment`): a tone in buffer 0 only, then in
//!    buffer 1 only, through ingress and egress of each canceller.  Expect
//!    each buffer to come back in place.  This settles whether buffer 1 of
//!    the ingress result is what the stream's combine is to be given.
//! 3. **Receive gain registers**: with a constant tone through the same
//!    command (egress), write HEC register 4 and SBAEC register 2 with
//!    -9, -3, and +3 dB (times 256) using `dphone dsp job`, and measure
//!    the result's level.
//!    Then the same with SBAEC register 0 set to 0, 1, and a gain value, to
//!    learn what that register does.
//! 4. **Job order within a tick** (new family): in a speakerphone call,
//!    compare the echo left in the ingress result (far-end tone, quiet
//!    room) for "egress, combine, split, ingress" (what the engine does)
//!    against "combine, split, ingress, egress" (the order the jobs were
//!    listed in the traces, which would play the previous tick's egress
//!    result).
//! 5. **Headset canceller**: a headset call; HEC instance 1 is assumed.
//! 6. **G.726 byte order**: encode a known file through the DSP and compare
//!    with a reference G.726-32 encoder in both nibble orders, with and
//!    without the pair swap that G.722 needs.
//! 7. **Lost frames**: a decode job with frame type 6 and no data after
//!    normal frames (`VpmDspOptions::plc_frame_type_6`); see whether the
//!    output is a concealment or an error.
//! 8. **Old family filter bank**: a D40 speakerphone call; measure echo
//!    with the ARM-side filter bank here, since the vendor's filters (and
//!    so the delay between the two buffers the canceller was tuned for)
//!    are not known.
//! 9. **Listening**: nobody has heard mainline audio yet, and capture
//!    samples from a D60 looked wrong (full-scale peaks, DC offset); the
//!    engine assumes signed 16-bit samples at 32 kHz (new) or 16 kHz (old).

pub mod bands;
pub mod dtmf;
pub mod jitter;
pub mod ring;
pub mod rtp;
pub mod srtp;
pub mod tone;
pub mod device;
pub mod engine;
pub mod fake;
pub mod net;
pub mod softdsp;
pub mod vpmdsp;

pub use bands::BandFrame;
pub use device::{AudioDevice, AudioPath, Dsp, GainControl};
pub use engine::{
    Command, DtmfMode, Engine, EngineCore, EngineOptions, Event, Meters, PayloadCodec, SrtpConfig, StreamConfig, StreamStats,
};
pub use engine::Law;
pub use softdsp::SoftDsp;
pub use vpmdsp::{VpmDsp, VpmDspOptions};
