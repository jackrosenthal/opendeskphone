// SPDX-License-Identifier: GPL-2.0-or-later
//! Ring playback: ringtones and alarm tones, 16 kHz samples looped with a
//! pause between passes, and the decoding of the WAV files (PCM, mu-law,
//! or A-law) they come in.  The same player plays a sound file once for an
//! app ([`RingSource::Once`]).  Output is 16 kHz mono, 10 ms at a time.

use std::sync::Arc;

use crate::bands::WB;
use crate::engine::Law;

/// Samples per second of everything the player plays.
pub const RING_RATE: u32 = 16_000;

/// What to ring with.
#[derive(Clone, Debug)]
pub enum RingSource {
    /// 16 kHz samples, looped with a pause between passes.
    Pcm { samples: Arc<[i16]>, pause_ms: u32 },
    /// 16 kHz samples played once, from sample `start` to the end.
    /// `token` names the playback in [`RingPlayer::one_shot`].
    Once { samples: Arc<[i16]>, start: usize, token: u32 },
}

#[derive(Clone, Debug)]
enum State {
    Pcm { samples: Arc<[i16]>, pos: usize, pause: u32, pause_left: u32 },
    Once { samples: Arc<[i16]>, pos: usize, token: u32 },
}

/// Produces a ring, 160 samples (10 ms at 16 kHz) at a time.
#[derive(Clone, Debug)]
pub struct RingPlayer {
    state: State,
    /// Output gain, Q12.
    gain_q12: i32,
}

/// Where [`soft_limit`] starts to bend, and what it never exceeds.
const LIMIT_KNEE: i32 = 20_000;
const LIMIT_CEILING: i32 = 32_000;

/// Leaves samples up to [`LIMIT_KNEE`] alone and bends louder ones over
/// smoothly (the slope is continuous at the knee) so that none exceeds
/// [`LIMIT_CEILING`]: a ring turned up past full scale rounds off rather
/// than clips.
fn soft_limit(v: i32) -> i16 {
    let mag = v.abs();
    if mag <= LIMIT_KNEE {
        return v as i16;
    }
    let room = LIMIT_CEILING - LIMIT_KNEE;
    let over = i64::from(mag - LIMIT_KNEE);
    let out = LIMIT_KNEE + (i64::from(room) * over / (i64::from(room) + over)) as i32;
    (if v < 0 { -out } else { out }) as i16
}

impl RingPlayer {
    pub fn new(source: RingSource) -> Self {
        let state = match source {
            RingSource::Pcm { samples, pause_ms } => State::Pcm { samples, pos: 0, pause: pause_ms * (RING_RATE / 1000), pause_left: 0 },
            RingSource::Once { samples, start, token } => {
                let pos = start.min(samples.len());
                State::Once { samples, pos, token }
            }
        };
        Self { state, gain_q12: 4096 }
    }

    pub fn set_gain_q12(&mut self, gain: i32) {
        self.gain_q12 = gain;
    }

    /// For a [`RingSource::Once`] source: its token, the sample position
    /// reached, and whether it has played to the end.
    pub fn one_shot(&self) -> Option<(u32, usize, bool)> {
        match &self.state {
            State::Once { samples, pos, token } => Some((*token, *pos, *pos >= samples.len())),
            _ => None,
        }
    }

    /// Fills `out` with the next 10 ms.
    pub fn fill(&mut self, out: &mut [i16; WB]) {
        match &mut self.state {
            State::Pcm { samples, pos, pause, pause_left } => {
                for o in out.iter_mut() {
                    *o = if *pause_left > 0 || samples.is_empty() {
                        *pause_left = pause_left.saturating_sub(1);
                        0
                    } else {
                        let s = samples[*pos];
                        *pos += 1;
                        if *pos >= samples.len() {
                            *pos = 0;
                            *pause_left = *pause;
                        }
                        s
                    };
                }
            }
            State::Once { samples, pos, .. } => {
                let n = (samples.len() - *pos).min(WB);
                out[..n].copy_from_slice(&samples[*pos..*pos + n]);
                out[n..].fill(0);
                *pos += n;
            }
        }
        if self.gain_q12 <= 4096 {
            // Nothing can pass full scale.
            crate::bands::scale(out, self.gain_q12.max(0));
        } else {
            let gain = self.gain_q12.min(8 << 12);
            for o in out.iter_mut() {
                *o = soft_limit((i32::from(*o) * gain) >> 12);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WavError {
    NotWav,
    Truncated,
    /// Format tag, channels, or sample size this player does not handle.
    Unsupported(&'static str),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavError::NotWav => f.write_str("not a RIFF/WAVE file"),
            WavError::Truncated => f.write_str("truncated WAVE file"),
            WavError::Unsupported(what) => write!(f, "unsupported WAVE file: {what}"),
        }
    }
}

impl std::error::Error for WavError {}

/// Decodes a WAV file (8-bit or 16-bit PCM, mu-law, or A-law; mono or the
/// first channel of more) to 16 kHz mono.  Not for the audio thread: it
/// allocates, and it resamples by linear interpolation.
pub fn decode_wav(data: &[u8]) -> Result<Vec<i16>, WavError> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    let mut pcm: Option<&[u8]> = None;
    let mut o = 12;
    while o + 8 <= data.len() {
        let id = &data[o..o + 4];
        let len = u32_at(data, o + 4) as usize;
        let body = &data[o + 8..data.len().min(o + 8 + len)];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(WavError::Truncated);
                }
                fmt = Some((u16_at(body, 0), usize::from(u16_at(body, 2)), u32_at(body, 4), u16_at(body, 14)));
            }
            b"data" => pcm = Some(body),
            _ => {}
        }
        o += 8 + len + (len & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or(WavError::NotWav)?;
    let pcm = pcm.ok_or(WavError::Truncated)?;
    if channels == 0 || rate == 0 || rate > 192_000 {
        return Err(WavError::Unsupported("channel count or sample rate"));
    }
    let frame = channels * usize::from(bits / 8);
    let first: Vec<i16> = match (tag, bits) {
        (1, 16) => pcm.chunks_exact(frame).map(|f| i16::from_le_bytes([f[0], f[1]])).collect(),
        (1, 8) => pcm.chunks_exact(frame).map(|f| (i16::from(f[0]) - 128) << 8).collect(),
        (7 | 6, 8) => {
            let coded: Vec<u8> = pcm.chunks_exact(frame).map(|f| f[0]).collect();
            let mut out = vec![0i16; coded.len()];
            (if tag == 7 { Law::Mu } else { Law::A }).decode(&coded, &mut out);
            out
        }
        _ => return Err(WavError::Unsupported("sample format")),
    };
    if rate == RING_RATE || first.is_empty() {
        return Ok(first);
    }
    // linear interpolation to 16 kHz, positions in Q16
    let out_len = (first.len() as u64 * u64::from(RING_RATE) / u64::from(rate)) as usize;
    let step = (u64::from(rate) << 16) / u64::from(RING_RATE);
    Ok((0..out_len)
        .map(|i| {
            let p = i as u64 * step;
            let (idx, frac) = ((p >> 16) as usize, (p & 0xffff) as i32);
            let a = i32::from(first[idx.min(first.len() - 1)]);
            let b = i32::from(first[(idx + 1).min(first.len() - 1)]);
            (a + (((b - a) * frac) >> 16)) as i16
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&tag.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        w.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        w.extend_from_slice(&bits.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(data);
        w
    }

    #[test]
    fn wav_formats() {
        let pcm: Vec<u8> = [100i16, -200, 300, -400].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(decode_wav(&wav(1, 1, 16_000, 16, &pcm)).unwrap(), [100, -200, 300, -400]);
        // stereo: first channel
        assert_eq!(decode_wav(&wav(1, 2, 16_000, 16, &pcm)).unwrap(), [100, 300]);
        // 8 kHz is brought to 16 kHz
        let up = decode_wav(&wav(1, 1, 8_000, 16, &pcm)).unwrap();
        assert_eq!(up.len(), 8);
        assert_eq!((up[0], up[1], up[2]), (100, -50, -200));
        // mu-law
        let mut mu = [0u8; 2];
        Law::Mu.encode(&[1000, -1000], &mut mu);
        let d = decode_wav(&wav(7, 1, 16_000, 8, &mu)).unwrap();
        assert!((d[0] - 1000).abs() < 40 && (d[1] + 1000).abs() < 40);
        assert_eq!(decode_wav(b"nope"), Err(WavError::NotWav));
        assert_eq!(decode_wav(&wav(3, 1, 16_000, 32, &pcm)), Err(WavError::Unsupported("sample format")));
    }

    /// A loud 1 kHz pass of 0.1 s (peak 26,000, the loudest a ringtone is
    /// stored at).
    fn pass() -> Arc<[i16]> {
        (0..1600).map(|i| ((f64::from(i) * std::f64::consts::TAU / 16.0).sin() * 26_000.0) as i16).collect()
    }

    /// [`pass`] and a 0.1 s pause, through the player at `gain_q12`.
    fn render(gain_q12: i32) -> Vec<i16> {
        let mut p = RingPlayer::new(RingSource::Pcm { samples: pass(), pause_ms: 100 });
        p.set_gain_q12(gain_q12);
        let mut pcm = Vec::new();
        for _ in 0..20 {
            let mut buf = [0i16; WB];
            p.fill(&mut buf);
            pcm.extend_from_slice(&buf);
        }
        pcm
    }

    #[test]
    fn a_loud_ring_is_limited_not_clipped() {
        assert_eq!(soft_limit(0), 0);
        assert_eq!(soft_limit(LIMIT_KNEE), LIMIT_KNEE as i16);
        assert_eq!(soft_limit(-12_345), -12_345);
        let mut last = 0;
        for v in (0..400_000).step_by(7) {
            let out = i32::from(soft_limit(v));
            assert!(out >= last && out <= LIMIT_CEILING, "{v} -> {out}");
            assert_eq!(i32::from(soft_limit(-v)), -out);
            last = out;
        }
        // The top of the volume scale is 15 dB above unity: rounded off,
        // and still louder than one step down.
        let peak = |pcm: &[i16]| pcm.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        let top = render(23_033);
        assert!(peak(&top) > 30_000 && peak(&top) <= LIMIT_CEILING, "{}", peak(&top));
        assert!(peak(&render(16_306)) < peak(&top));
        // At unity and below, the samples are the stored ones, scaled.
        let unity = render(4096);
        assert_eq!(unity[..1600], pass()[..]);
        let quiet = render(2048);
        assert!(quiet.iter().zip(&unity).all(|(q, u)| (i32::from(*q) - i32::from(*u) / 2).abs() <= 1));
        assert!(render(0).iter().all(|&s| s == 0));
    }

    #[test]
    fn one_shot_plays_from_its_start_to_the_end() {
        let samples: Arc<[i16]> = (0..400).map(|i| i as i16 + 1).collect();
        let mut p = RingPlayer::new(RingSource::Once { samples: samples.clone(), start: 100, token: 7 });
        assert_eq!(p.one_shot(), Some((7, 100, false)));
        let mut buf = [0i16; WB];
        p.fill(&mut buf);
        assert_eq!((buf[0], buf[159]), (101, 260));
        assert_eq!(p.one_shot(), Some((7, 260, false)));
        p.fill(&mut buf);
        assert_eq!((buf[0], buf[139]), (261, 400));
        assert!(buf[140..].iter().all(|&s| s == 0));
        assert_eq!(p.one_shot(), Some((7, 400, true)));
        p.fill(&mut buf);
        assert!(buf.iter().all(|&s| s == 0));
        // A start beyond the end is the end.
        let p = RingPlayer::new(RingSource::Once { samples, start: 9000, token: 1 });
        assert_eq!(p.one_shot(), Some((1, 400, true)));
        assert_eq!(RingPlayer::new(RingSource::Pcm { samples: Arc::from([1i16]), pause_ms: 0 }).one_shot(), None);
    }

    #[test]
    fn pcm_ring_loops() {
        let samples: Arc<[i16]> = (0..240).map(|i| i as i16 + 1).collect();
        let mut p = RingPlayer::new(RingSource::Pcm { samples, pause_ms: 10 });
        let mut buf = [0i16; WB];
        p.fill(&mut buf);
        assert_eq!(buf[159], 160);
        p.fill(&mut buf);
        assert_eq!(buf[79], 240);
        assert!(buf[80..].iter().all(|&s| s == 0));
        p.fill(&mut buf);
        assert!(buf[..80].iter().all(|&s| s == 0));
        assert_eq!(buf[80], 1);
    }
}
