// SPDX-License-Identifier: GPL-2.0-or-later
//! The engine's DSP for a phone that has none to give jobs to: the D80.
//!
//! The D40 thru D70 have a voice DSP beside the ARM, which runs the echo
//! cancellers, the receive gain, and the codecs other than G.711
//! ([`crate::vpmdsp`]).  The D80 has a voice processor of another kind, a
//! ZL38040, between the SoC and the microphones and loudspeakers: it cancels
//! the echo by itself, on every path, and nothing is sent to it but the
//! audio.  So what the engine asks of a DSP is done here, on the ARM, or
//! not at all:
//!
//! - The band buffers: the two-band filter bank of [`crate::bands`], as
//!   for the old family, whose front end is at 16 kHz too.
//! - The cancellers: nothing.  The audio passes as it is.
//! - The receive gain: applied to the samples on their way to the front
//!   end.  The voice processor takes them as the far end's signal, at the
//!   level they are played at.
//! - Codecs: G.722, with the `audio-codec` crate's coder, as the stock
//!   firmware runs it in software too (its PJSIP's).  No other: G.711 the
//!   engine runs itself, and G.726 has no coder here ([`SoftDsp::runs`] is
//!   what call setup asks).
//!
//! None of this has run on a D80.

use audio_codec::g722::{G722Decoder, G722Encoder};
use audio_codec::{Decoder, Encoder};
use opendeskphone_vpm::{Codec, Error, Family};

use crate::bands::{BandFrame, Combine2, SWB, Split2, WB, scale};
use crate::device::{AudioPath, Dsp, DspResult};

const SLOTS: usize = crate::vpmdsp::SLOTS;

/// A gain in dB * 256 as a factor in Q12 (4096 is unity).
pub fn gain_q12(db_q8: i16) -> i32 {
    (4096.0 * 10f64.powf(f64::from(db_q8) / 256.0 / 20.0)).round() as i32
}

/// See the module's description.
pub struct SoftDsp {
    /// Receive gain of each path, Q12.
    gains: [i32; 3],
    dev_split: Split2,
    dev_combine: Combine2,
    stream_split: [Split2; SLOTS],
    stream_combine: [Combine2; SLOTS],
    /// Each stream slot's G.722 coder, while it has one open.
    g722: [Option<Box<(G722Encoder, G722Decoder)>>; SLOTS],
}

/// G.722's frame as the engine codes it: 10 ms, 160 samples at 16 kHz in
/// 80 bytes.
const G722_SAMPLES: usize = WB;
const G722_BYTES: usize = WB / 2;

impl Default for SoftDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftDsp {
    pub fn new() -> SoftDsp {
        let g = opendeskphone_vpm::startup::tuning(Family::Old).gains;
        SoftDsp {
            gains: [g.handset, g.headset, g.speaker].map(gain_q12),
            dev_split: Split2::new(),
            dev_combine: Combine2::new(),
            stream_split: [const { Split2::new() }; SLOTS],
            stream_combine: [const { Combine2::new() }; SLOTS],
            g722: [const { None }; SLOTS],
        }
    }

    /// Whether `codec` (one the engine gives a DSP to run) can run here.
    pub const fn runs(codec: Codec) -> bool {
        matches!(codec, Codec::G722)
    }

    fn slot(slot: usize) -> DspResult<usize> {
        if slot < SLOTS { Ok(slot) } else { Err(Error::Invalid) }
    }
}

impl Dsp for SoftDsp {
    /// The old family's: a front end at 16 kHz, and the filter bank on the
    /// ARM.
    fn family(&self) -> Family {
        Family::Old
    }

    fn start(&mut self, _force: bool) -> DspResult<()> {
        Ok(())
    }

    fn startup_summary(&self) -> Option<String> {
        Some("no DSP: the filter bank and the receive gain run on the ARM".into())
    }

    fn attach(&mut self, path: AudioPath, rx_gain_q8: i16, _tone: bool) -> DspResult<()> {
        self.gains[path.index()] = gain_q12(rx_gain_q8);
        self.dev_split.reset();
        self.dev_combine.reset();
        Ok(())
    }

    fn detach(&mut self, _path: AudioPath) -> DspResult<()> {
        Ok(())
    }

    fn set_rx_gain(&mut self, path: AudioPath, rx_gain_q8: i16) -> DspResult<()> {
        self.gains[path.index()] = gain_q12(rx_gain_q8);
        Ok(())
    }

    fn set_tone_mode(&mut self, _path: AudioPath, _tone: bool) -> DspResult<()> {
        Ok(())
    }

    fn mic_to_bands(&mut self, _path: AudioPath, pcm: &[i16], out: &mut BandFrame) -> DspResult<()> {
        if pcm.len() != WB {
            return Err(Error::Invalid);
        }
        self.dev_split.run(pcm, &mut out.nb, &mut out.hi1);
        out.hi2 = [0; SWB];
        Ok(())
    }

    fn bands_to_speaker(&mut self, path: AudioPath, bands: &BandFrame, pcm: &mut [i16]) -> DspResult<()> {
        if pcm.len() != WB {
            return Err(Error::Invalid);
        }
        self.dev_combine.run(&bands.nb, &bands.hi1, pcm);
        scale(pcm, self.gains[path.index()]);
        Ok(())
    }

    fn ingress(&mut self, _path: AudioPath, _frame: &mut BandFrame) -> DspResult<()> {
        Ok(())
    }

    fn egress(&mut self, _path: AudioPath, _frame: &mut BandFrame) -> DspResult<()> {
        Ok(())
    }

    fn poll(&mut self, _path: AudioPath) -> DspResult<()> {
        Ok(())
    }

    fn wb_to_bands(&mut self, slot: usize, wb: &[i16; WB], out: &mut BandFrame) -> DspResult<()> {
        let slot = Self::slot(slot)?;
        self.stream_split[slot].run(wb, &mut out.nb, &mut out.hi1);
        out.hi2 = [0; SWB];
        Ok(())
    }

    fn bands_to_wb(&mut self, slot: usize, bands: &BandFrame, wb: &mut [i16; WB]) -> DspResult<()> {
        let slot = Self::slot(slot)?;
        self.stream_combine[slot].run(&bands.nb, &bands.hi1, wb);
        Ok(())
    }

    fn codec_open(&mut self, slot: usize, codec: Codec) -> DspResult<()> {
        let slot = Self::slot(slot)?;
        self.g722[slot] = None;
        if !Self::runs(codec) {
            return Err(Error::Unsupported("a codec other than G.711 and G.722 on a phone without the DSP"));
        }
        // 64 kbit/s, the one mode RTP carries.
        self.g722[slot] = Some(Box::new((G722Encoder::new(), G722Decoder::new())));
        self.stream_split[slot].reset();
        self.stream_combine[slot].reset();
        Ok(())
    }

    fn encode(&mut self, slot: usize, pcm: &[i16], out: &mut [u8]) -> DspResult<usize> {
        let coder = self.g722[Self::slot(slot)?].as_mut().ok_or(Error::Invalid)?;
        if pcm.len() != G722_SAMPLES || out.len() < G722_BYTES {
            return Err(Error::Invalid);
        }
        match coder.0.encode_into(pcm, &mut out[..G722_BYTES]) {
            Ok(G722_BYTES) => Ok(G722_BYTES),
            _ => Err(Error::Invalid),
        }
    }

    fn decode(&mut self, slot: usize, frame: &[u8], pcm: &mut [i16]) -> DspResult<usize> {
        let coder = self.g722[Self::slot(slot)?].as_mut().ok_or(Error::Invalid)?;
        if frame.len() != G722_BYTES || pcm.len() < G722_SAMPLES {
            return Err(Error::Invalid);
        }
        match coder.1.decode_into(frame, &mut pcm[..G722_SAMPLES]) {
            Ok(G722_SAMPLES) => Ok(G722_SAMPLES),
            _ => Err(Error::Invalid),
        }
    }

    /// A lost frame plays as silence, as on the phones with the DSP.
    fn decode_lost(&mut self, slot: usize, pcm: &mut [i16]) -> DspResult<usize> {
        if self.g722[Self::slot(slot)?].is_none() || pcm.len() < G722_SAMPLES {
            return Err(Error::Invalid);
        }
        pcm[..G722_SAMPLES].fill(0);
        Ok(G722_SAMPLES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::BANK_DELAY;

    #[test]
    fn gains_in_decibels_are_factors() {
        assert_eq!(gain_q12(0), 4096);
        // -6.02 dB is a half, +6.02 dB is double.
        assert!((gain_q12(-6 * 256) - 2053).abs() <= 1);
        assert!((gain_q12(6 * 256) - 8173).abs() <= 1);
        assert_eq!(gain_q12(-20 * 256), 410);
    }

    /// A tone through the microphone's split and the loudspeaker's combine
    /// comes back as it went in, later by the filter bank's delay, at the
    /// path's receive gain.
    #[test]
    fn audio_passes_at_the_receive_gain() {
        let tone: Vec<i16> = (0..4 * WB).map(|n| (8000.0 * (n as f64 * 2.0 * std::f64::consts::PI * 1000.0 / 16000.0).sin()) as i16).collect();
        for (db, factor) in [(0i16, 1.0f64), (-6, 0.5012)] {
            let mut dsp = SoftDsp::new();
            dsp.start(false).unwrap();
            dsp.attach(AudioPath::Speaker, db * 256, false).unwrap();
            let mut out = Vec::new();
            for block in tone.as_chunks::<WB>().0 {
                let mut bands = BandFrame::default();
                dsp.mic_to_bands(AudioPath::Speaker, block, &mut bands).unwrap();
                dsp.ingress(AudioPath::Speaker, &mut bands).unwrap();
                dsp.egress(AudioPath::Speaker, &mut bands).unwrap();
                let mut pcm = [0i16; WB];
                dsp.bands_to_speaker(AudioPath::Speaker, &bands, &mut pcm).unwrap();
                out.extend_from_slice(&pcm);
            }
            for n in WB..3 * WB {
                let want = f64::from(tone[n]) * factor;
                let got = f64::from(out[n + BANK_DELAY]);
                assert!((got - want).abs() < 12.0, "{db} dB, sample {n}: {got} for {want}");
            }
        }
    }

    /// A 1 kHz tone through the G.722 coder comes back a tone of about the
    /// same level, 10 ms at a time, and a slot's coder is its own.
    #[test]
    fn g722_codes_a_tone() {
        let mut dsp = SoftDsp::new();
        assert!(SoftDsp::runs(Codec::G722) && !SoftDsp::runs(Codec::G726_32));
        assert!(dsp.encode(0, &[0; WB], &mut [0; 80]).is_err());
        dsp.codec_open(0, Codec::G722).unwrap();
        let tone: Vec<i16> = (0..20 * WB).map(|n| (8000.0 * (n as f64 * 2.0 * std::f64::consts::PI * 1000.0 / 16000.0).sin()) as i16).collect();
        let mut back = Vec::new();
        for block in tone.as_chunks::<WB>().0 {
            let mut coded = [0u8; 80];
            assert_eq!(dsp.encode(0, block, &mut coded).unwrap(), 80);
            let mut pcm = [0i16; WB];
            assert_eq!(dsp.decode(0, &coded, &mut pcm).unwrap(), WB);
            back.extend_from_slice(&pcm);
        }
        let rms = |x: &[i16]| (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let (sent, got) = (rms(&tone[4 * WB..]), rms(&back[4 * WB..]));
        assert!((got / sent - 1.0).abs() < 0.1, "level {got} for {sent}");
        // The tone is still at 1 kHz: 16 samples a cycle, so a sample and
        // the one a cycle later agree.
        let apart = back[4 * WB..].windows(17).map(|w| f64::from(w[16] - w[0]).abs()).sum::<f64>() / (16 * WB) as f64;
        assert!(apart < 400.0, "not periodic: {apart}");
        // A lost frame is silence; another slot has no coder; a wrong
        // length is refused.
        let mut pcm = [1i16; WB];
        assert_eq!(dsp.decode_lost(0, &mut pcm).unwrap(), WB);
        assert_eq!(pcm, [0; WB]);
        assert!(dsp.decode(1, &[0; 80], &mut pcm).is_err());
        assert!(dsp.decode(0, &[0; 40], &mut pcm).is_err());
    }

    #[test]
    fn other_codecs_do_not_open() {
        let mut dsp = SoftDsp::new();
        assert!(dsp.codec_open(0, Codec::G726_32).is_err());
        assert_eq!(dsp.family().device_rate(), 16_000);
        // A wrong frame length is refused, as by the other DSP.
        let mut bands = BandFrame::default();
        assert!(dsp.mic_to_bands(AudioPath::Handset, &[0; 320], &mut bands).is_err());
    }
}
