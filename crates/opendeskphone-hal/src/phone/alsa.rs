//! ALSA PCM and control devices by raw ioctl (no alsa-lib).
//!
//! Card "Digium Phone": PCM 0 speakerphone, 1 handset, 2 headset; each
//! mono S16_LE at the family rate.  Only blocking `READI`/`WRITEI` are
//! used, which keeps clear of the ioctls whose layout depends on the
//! 64-bit time ABI.
//!
//! The D80's card (its ZL38040 voice processor) is otherwise: one PCM
//! device for every path, and a mixer control, `Audio Path`, that selects
//! the path.  The device is stereo, with the voice in the left channel in
//! both directions (`kernel/README.md`); the samples here are mono all the
//! same, as on the other models.

#![allow(clippy::unnecessary_cast)] // c_long is i32 on ARM, i64 on the host

use super::sys::*;
use crate::*;
use libc::{c_long, c_ulong};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const CARD_NAME: &str = "Digium Phone";

// ---- PCM structures

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SndMask {
    bits: [u32; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SndInterval {
    min: u32,
    max: u32,
    /// bit 0 openmin, 1 openmax, 2 integer, 3 empty.
    flags: u32,
}

#[repr(C)]
pub(crate) struct SndPcmHwParams {
    flags: u32,
    masks: [SndMask; 3],
    mres: [SndMask; 5],
    pub(crate) intervals: [SndInterval; 12],
    ires: [SndInterval; 9],
    pub(crate) rmask: u32,
    cmask: u32,
    info: u32,
    msbits: u32,
    rate_num: u32,
    rate_den: u32,
    pub(crate) fifo_size: c_ulong,
    reserved: [u8; 64],
}

#[repr(C)]
pub(crate) struct SndPcmSwParams {
    tstamp_mode: i32,
    period_step: u32,
    sleep_min: u32,
    avail_min: c_ulong,
    xfer_align: c_ulong,
    start_threshold: c_ulong,
    stop_threshold: c_ulong,
    silence_threshold: c_ulong,
    silence_size: c_ulong,
    boundary: c_ulong,
    pub(crate) proto: u32,
    tstamp_type: u32,
    reserved: [u8; 56],
}

#[repr(C)]
pub(crate) struct SndXferi {
    result: c_long,
    buf: *mut libc::c_void,
    frames: c_ulong,
}

pub(crate) const SNDRV_PCM_IOCTL_HW_PARAMS: u32 = iowr::<SndPcmHwParams>(b'A', 0x11);
pub(crate) const SNDRV_PCM_IOCTL_SW_PARAMS: u32 = iowr::<SndPcmSwParams>(b'A', 0x13);
pub(crate) const SNDRV_PCM_IOCTL_DELAY: u32 = ior::<c_long>(b'A', 0x21);
pub(crate) const SNDRV_PCM_IOCTL_PREPARE: u32 = io(b'A', 0x40);
pub(crate) const SNDRV_PCM_IOCTL_DROP: u32 = io(b'A', 0x43);
pub(crate) const SNDRV_PCM_IOCTL_WRITEI_FRAMES: u32 = iow::<SndXferi>(b'A', 0x50);
pub(crate) const SNDRV_PCM_IOCTL_READI_FRAMES: u32 = ior::<SndXferi>(b'A', 0x51);

const HW_PARAM_ACCESS: usize = 0;
const HW_PARAM_FORMAT: usize = 1;
const HW_PARAM_SUBFORMAT: usize = 2;
const HW_PARAM_FIRST_INTERVAL: usize = 8;
const HW_PARAM_CHANNELS: usize = 10;
const HW_PARAM_RATE: usize = 11;
const HW_PARAM_PERIOD_SIZE: usize = 13;
const HW_PARAM_PERIODS: usize = 15;
const HW_PARAM_BUFFER_SIZE: usize = 17;
const ACCESS_RW_INTERLEAVED: u32 = 3;
const FORMAT_S16_LE: u32 = 2;
const SUBFORMAT_STD: u32 = 0;

impl SndPcmHwParams {
    /// "Anything goes": all masks full, all intervals 0..=MAX, as alsa-lib
    /// and tinyalsa start out.
    fn any() -> Box<SndPcmHwParams> {
        // SAFETY: the struct is plain integers; all-zero is valid.
        let mut p: Box<SndPcmHwParams> = Box::new(unsafe { std::mem::zeroed() });
        for m in p.masks.iter_mut() {
            m.bits = [u32::MAX; 8];
        }
        for i in p.intervals.iter_mut() {
            i.min = 0;
            i.max = u32::MAX;
            i.flags = 0;
        }
        p.rmask = u32::MAX;
        p.info = u32::MAX;
        p
    }
    fn set_mask(&mut self, param: usize, bit: u32) {
        let m = &mut self.masks[param];
        m.bits = [0; 8];
        m.bits[(bit / 32) as usize] = 1 << (bit % 32);
    }
    fn set_int(&mut self, param: usize, v: u32) {
        let i = &mut self.intervals[param - HW_PARAM_FIRST_INTERVAL];
        i.min = v;
        i.max = v;
        i.flags = 1 << 2;
    }
    fn get_int(&self, param: usize) -> u32 {
        self.intervals[param - HW_PARAM_FIRST_INTERVAL].min
    }
}

fn pcm_err(what: &str, e: std::io::Error) -> HalError {
    match e.raw_os_error() {
        Some(libc::EPIPE) | Some(libc::ESTRPIPE) => HalError::Xrun,
        _ => HalError::io(format!("PCM {what}"), e),
    }
}

struct Stream {
    file: File,
    /// Samples a frame: 1, or on the D80 2.
    channels: usize,
    buffer_frames: u32,
    /// The last transfer ended in an overrun or underrun.
    xrun: bool,
}

/// Queued frames at which playback starts: half the buffer, in whole
/// periods.  The audio loop writes one tick each time the capture side has
/// delivered one, so with a threshold of one write the queue is empty again
/// just as the next write is due, and any tick that takes longer than the
/// first one underruns (measured on a D60 and a D40: playback in XRUN and
/// the capture stream restarted about nine times a second, 91 ticks a
/// second in place of 100).  Starting at half the buffer keeps that much
/// queued, and leaves the other half for a tick that comes early.
fn playback_start_threshold(period: u32, buffer: u32) -> u32 {
    let period = period.max(1);
    ((buffer / 2) / period).max(1) * period
}

impl Stream {
    fn open(card: u32, device: u32, channels: usize, playback: bool, rate: u32, cfg: &PcmConfig) -> Result<(Stream, u32, u32)> {
        let path = PathBuf::from(format!("/dev/snd/pcmC{card}D{device}{}", if playback { 'p' } else { 'c' }));
        // Non-blocking open so a busy device fails instead of hanging;
        // then switch to blocking I/O.
        let file = open_rw(&path, libc::O_NONBLOCK).map_err(|e| HalError::io(path.display().to_string(), e))?;
        set_blocking(&file).map_err(|e| HalError::io("fcntl", e))?;
        let fd = file.as_raw_fd();

        let mut hw = SndPcmHwParams::any();
        hw.set_mask(HW_PARAM_ACCESS, ACCESS_RW_INTERLEAVED);
        hw.set_mask(HW_PARAM_FORMAT, FORMAT_S16_LE);
        hw.set_mask(HW_PARAM_SUBFORMAT, SUBFORMAT_STD);
        hw.set_int(HW_PARAM_CHANNELS, channels as u32);
        hw.set_int(HW_PARAM_RATE, rate);
        hw.set_int(HW_PARAM_PERIOD_SIZE, cfg.period_frames);
        hw.set_int(HW_PARAM_PERIODS, cfg.periods.max(2));
        if let Err(first) = ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &mut *hw) {
            // The driver may not allow that exact geometry: ask only for the
            // format and rate with at least our period, and take what comes.
            let mut hw2 = SndPcmHwParams::any();
            hw2.set_mask(HW_PARAM_ACCESS, ACCESS_RW_INTERLEAVED);
            hw2.set_mask(HW_PARAM_FORMAT, FORMAT_S16_LE);
            hw2.set_mask(HW_PARAM_SUBFORMAT, SUBFORMAT_STD);
            hw2.set_int(HW_PARAM_CHANNELS, channels as u32);
            hw2.set_int(HW_PARAM_RATE, rate);
            hw2.intervals[HW_PARAM_PERIOD_SIZE - HW_PARAM_FIRST_INTERVAL].min = cfg.period_frames;
            if ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &mut *hw2).is_err() {
                return Err(HalError::io(format!("PCM HW_PARAMS {}", path.display()), first));
            }
            hw = hw2;
        }
        let period = hw.get_int(HW_PARAM_PERIOD_SIZE);
        let buffer = hw.get_int(HW_PARAM_BUFFER_SIZE);
        let periods = hw.get_int(HW_PARAM_PERIODS);

        // Same computation as the kernel's.
        let mut boundary = buffer as c_ulong;
        while boundary * 2 <= c_long::MAX as c_ulong - buffer as c_ulong {
            boundary *= 2;
        }
        // SAFETY: plain integers; zero is valid.
        let mut sw: SndPcmSwParams = unsafe { std::mem::zeroed() };
        sw.period_step = 1;
        sw.avail_min = period as c_ulong;
        // Capture starts on the first read; playback once half the buffer
        // is queued (see `playback_start_threshold`).
        sw.start_threshold = if playback { playback_start_threshold(period, buffer) as c_ulong } else { 1 };
        sw.stop_threshold = buffer as c_ulong;
        sw.boundary = boundary;
        ioctl(fd, SNDRV_PCM_IOCTL_SW_PARAMS, &mut sw).map_err(|e| HalError::io("PCM SW_PARAMS", e))?;
        ioctl_none(fd, SNDRV_PCM_IOCTL_PREPARE).map_err(|e| HalError::io("PCM PREPARE", e))?;
        Ok((Stream { file, channels, buffer_frames: buffer, xrun: false }, period, periods))
    }

    /// Transfers `frames` frames; `ptr` covers `frames * self.channels`
    /// samples.
    fn xfer(&mut self, req: u32, what: &str, ptr: *mut i16, frames: usize) -> Result<()> {
        let mut done = 0usize;
        while done < frames {
            let mut x = SndXferi {
                result: 0,
                // SAFETY of the pointer: `ptr` covers `frames` frames, and
                // `done < frames`.
                buf: unsafe { ptr.add(done * self.channels) } as *mut libc::c_void,
                frames: (frames - done) as c_ulong,
            };
            if let Err(e) = ioctl(self.file.as_raw_fd(), req, &mut x) {
                let e = pcm_err(what, e);
                self.xrun = matches!(e, HalError::Xrun);
                return Err(e);
            }
            if x.result <= 0 {
                return Err(HalError::io(format!("PCM {what}"), std::io::ErrorKind::UnexpectedEof.into()));
            }
            done += x.result as usize;
        }
        Ok(())
    }
}

struct AlsaPcm {
    device: PcmDevice,
    config: PcmConfig,
    capture: Option<Stream>,
    playback: Option<Stream>,
    open_set: Arc<Mutex<[bool; 3]>>,
    /// The frames of a stereo device, for a transfer of mono samples.
    stereo: Vec<i16>,
}

/// Mono samples as the frames of the D80's stereo device: the voice goes
/// in the left channel, and the right, which the voice processor does not
/// play, gets the same.
pub(crate) fn mono_to_stereo(mono: &[i16], stereo: &mut Vec<i16>) {
    stereo.clear();
    stereo.extend(mono.iter().flat_map(|s| [*s, *s]));
}

/// The left channel of the D80's stereo frames: the voice.  The right
/// channel is not driven.
pub(crate) fn stereo_to_mono(stereo: &[i16], mono: &mut [i16]) {
    for (m, frame) in mono.iter_mut().zip(stereo.as_chunks::<2>().0) {
        *m = frame[0];
    }
}

impl Pcm for AlsaPcm {
    fn config(&self) -> PcmConfig {
        self.config
    }

    fn read(&mut self, buf: &mut [i16]) -> Result<()> {
        let s = self.capture.as_mut().ok_or_else(|| HalError::Invalid("PCM not opened for capture".into()))?;
        if s.channels == 2 {
            self.stereo.resize(2 * buf.len(), 0);
            s.xfer(SNDRV_PCM_IOCTL_READI_FRAMES, "read", self.stereo.as_mut_ptr(), buf.len())?;
            stereo_to_mono(&self.stereo, buf);
            return Ok(());
        }
        s.xfer(SNDRV_PCM_IOCTL_READI_FRAMES, "read", buf.as_mut_ptr(), buf.len())
    }

    fn write(&mut self, buf: &[i16]) -> Result<()> {
        let s = self.playback.as_mut().ok_or_else(|| HalError::Invalid("PCM not opened for playback".into()))?;
        if s.channels == 2 {
            mono_to_stereo(buf, &mut self.stereo);
            return s.xfer(SNDRV_PCM_IOCTL_WRITEI_FRAMES, "write", self.stereo.as_mut_ptr(), buf.len());
        }
        // The kernel only reads through this pointer for WRITEI.
        s.xfer(SNDRV_PCM_IOCTL_WRITEI_FRAMES, "write", buf.as_ptr() as *mut i16, buf.len())
    }

    fn recover(&mut self) -> Result<()> {
        // Only the direction that failed: restarting the capture stream
        // for a playback underrun throws away what it had captured, and
        // with it the loop's clock.  Without a failed transfer on record,
        // both.
        let failed = [self.capture.as_ref(), self.playback.as_ref()].into_iter().flatten().any(|s| s.xrun);
        for s in [self.capture.as_mut(), self.playback.as_mut()].into_iter().flatten() {
            if failed && !s.xrun {
                continue;
            }
            s.xrun = false;
            let fd = s.file.as_raw_fd();
            let _ = ioctl_none(fd, SNDRV_PCM_IOCTL_DROP);
            ioctl_none(fd, SNDRV_PCM_IOCTL_PREPARE).map_err(|e| HalError::io("PCM PREPARE", e))?;
        }
        Ok(())
    }

    fn playback_delay(&mut self) -> Result<u32> {
        let s = self.playback.as_ref().ok_or_else(|| HalError::Invalid("PCM not opened for playback".into()))?;
        let mut d: c_long = 0;
        ioctl(s.file.as_raw_fd(), SNDRV_PCM_IOCTL_DELAY, &mut d).map_err(|e| pcm_err("delay", e))?;
        Ok(d.clamp(0, s.buffer_frames as c_long) as u32)
    }
}

impl Drop for AlsaPcm {
    fn drop(&mut self) {
        for s in [self.capture.as_ref(), self.playback.as_ref()].into_iter().flatten() {
            let _ = ioctl_none(s.file.as_raw_fd(), SNDRV_PCM_IOCTL_DROP);
        }
        self.open_set.lock().unwrap()[self.device.index() as usize] = false;
    }
}

// ---- control structures

#[repr(C)]
pub(crate) struct SndCtlCardInfo {
    card: i32,
    pad: i32,
    id: [u8; 16],
    driver: [u8; 16],
    name: [u8; 32],
    longname: [u8; 80],
    reserved: [u8; 16],
    mixername: [u8; 80],
    components: [u8; 128],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SndCtlElemId {
    numid: u32,
    iface: i32,
    device: u32,
    subdevice: u32,
    name: [u8; 44],
    index: u32,
}

#[repr(C)]
pub(crate) struct SndCtlElemList {
    offset: u32,
    space: u32,
    used: u32,
    count: u32,
    pids: *mut SndCtlElemId,
    reserved: [u8; 50],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InfoInteger {
    min: c_long,
    max: c_long,
    step: c_long,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InfoInteger64 {
    min: i64,
    max: i64,
    step: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InfoEnumerated {
    items: u32,
    item: u32,
    name: [u8; 64],
    names_ptr: u64,
    names_length: u32,
}

#[repr(C)]
pub(crate) union InfoValue {
    integer: InfoInteger,
    integer64: InfoInteger64,
    enumerated: InfoEnumerated,
    reserved: [u8; 128],
}

#[repr(C)]
pub(crate) struct SndCtlElemInfo {
    id: SndCtlElemId,
    type_: i32,
    access: u32,
    count: u32,
    owner: i32,
    pub(crate) value: InfoValue,
    reserved: [u8; 64],
}

#[repr(C)]
pub(crate) union ElemValueUnion {
    integer: [c_long; 128],
    integer64: [i64; 64],
    enumerated: [u32; 128],
    bytes: [u8; 512],
}

#[repr(C)]
pub(crate) struct SndCtlElemValue {
    id: SndCtlElemId,
    indirect: u32,
    pub(crate) value: ElemValueUnion,
    reserved: [u8; 128],
}

pub(crate) const SNDRV_CTL_IOCTL_CARD_INFO: u32 = ior::<SndCtlCardInfo>(b'U', 0x01);
pub(crate) const SNDRV_CTL_IOCTL_ELEM_LIST: u32 = iowr::<SndCtlElemList>(b'U', 0x10);
pub(crate) const SNDRV_CTL_IOCTL_ELEM_INFO: u32 = iowr::<SndCtlElemInfo>(b'U', 0x11);
pub(crate) const SNDRV_CTL_IOCTL_ELEM_READ: u32 = iowr::<SndCtlElemValue>(b'U', 0x12);
pub(crate) const SNDRV_CTL_IOCTL_ELEM_WRITE: u32 = iowr::<SndCtlElemValue>(b'U', 0x13);

const ELEM_TYPE_BOOLEAN: i32 = 1;
const ELEM_TYPE_INTEGER: i32 = 2;
const ELEM_TYPE_ENUMERATED: i32 = 3;

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn open_control(card: u32) -> std::io::Result<File> {
    open_rw(&PathBuf::from(format!("/dev/snd/controlC{card}")), 0)
}

/// Finds the phone's card number: the one named "Digium Phone", else the
/// first card present.
fn find_card() -> Result<u32> {
    let mut first = None;
    for n in 0..8 {
        let Ok(f) = open_control(n) else { continue };
        first.get_or_insert(n);
        // SAFETY: plain bytes and integers; zero is valid.
        let mut info: SndCtlCardInfo = unsafe { std::mem::zeroed() };
        if ioctl(f.as_raw_fd(), SNDRV_CTL_IOCTL_CARD_INFO, &mut info).is_ok() && cstr(&info.name) == CARD_NAME {
            return Ok(n);
        }
    }
    first.ok_or_else(|| HalError::Unavailable("no ALSA card".into()))
}

/// The number of the card with this name, if present.
fn find_card_named(name: &str) -> Option<u32> {
    (0..8).find(|n| {
        let Ok(f) = open_control(*n) else { return false };
        // SAFETY: plain bytes and integers; zero is valid.
        let mut info: SndCtlCardInfo = unsafe { std::mem::zeroed() };
        ioctl(f.as_raw_fd(), SNDRV_CTL_IOCTL_CARD_INFO, &mut info).is_ok() && cstr(&info.name) == name
    })
}

struct Ctl {
    id: SndCtlElemId,
    info: ControlInfo,
    type_: i32,
}

struct AlsaMixer {
    file: File,
    ctls: Vec<Ctl>,
}

impl AlsaMixer {
    fn open(card: u32) -> Result<AlsaMixer> {
        let file = open_control(card).map_err(|e| HalError::io(format!("/dev/snd/controlC{card}"), e))?;
        let fd = file.as_raw_fd();
        // SAFETY (all zeroed structs below): plain data; zero is valid.
        let mut list: SndCtlElemList = unsafe { std::mem::zeroed() };
        ioctl(fd, SNDRV_CTL_IOCTL_ELEM_LIST, &mut list).map_err(|e| HalError::io("ELEM_LIST", e))?;
        let mut ids: Vec<SndCtlElemId> = vec![unsafe { std::mem::zeroed() }; list.count as usize];
        let mut list2: SndCtlElemList = unsafe { std::mem::zeroed() };
        list2.space = ids.len() as u32;
        list2.pids = ids.as_mut_ptr();
        ioctl(fd, SNDRV_CTL_IOCTL_ELEM_LIST, &mut list2).map_err(|e| HalError::io("ELEM_LIST", e))?;
        ids.truncate(list2.used as usize);

        let mut ctls = Vec::new();
        for id in ids {
            let mut ei: SndCtlElemInfo = unsafe { std::mem::zeroed() };
            ei.id = id;
            if ioctl(fd, SNDRV_CTL_IOCTL_ELEM_INFO, &mut ei).is_err() {
                continue;
            }
            let (kind, min, max, mut items) = match ei.type_ {
                ELEM_TYPE_BOOLEAN => (ControlKind::Boolean, 0, 1, Vec::new()),
                // SAFETY: the kernel filled the member matching `type_`.
                ELEM_TYPE_INTEGER => unsafe {
                    (ControlKind::Integer, ei.value.integer.min as i32, ei.value.integer.max as i32, Vec::new())
                },
                ELEM_TYPE_ENUMERATED => unsafe {
                    (ControlKind::Enumerated, 0, ei.value.enumerated.items as i32 - 1, Vec::new())
                },
                _ => (ControlKind::Other, 0, 0, Vec::new()),
            };
            if kind == ControlKind::Enumerated {
                for item in 0..=max.max(0) as u32 {
                    let mut e2: SndCtlElemInfo = unsafe { std::mem::zeroed() };
                    e2.id = id;
                    e2.value.enumerated = InfoEnumerated { items: 0, item, name: [0; 64], names_ptr: 0, names_length: 0 };
                    if ioctl(fd, SNDRV_CTL_IOCTL_ELEM_INFO, &mut e2).is_ok() {
                        // SAFETY: enumerated info was returned.
                        items.push(cstr(unsafe { &e2.value.enumerated.name }));
                    }
                }
            }
            ctls.push(Ctl {
                id: ei.id,
                info: ControlInfo { name: cstr(&ei.id.name), kind, count: ei.count, min, max, items },
                type_: ei.type_,
            });
        }
        Ok(AlsaMixer { file, ctls })
    }

    fn find(&self, name: &str) -> Result<&Ctl> {
        self.ctls
            .iter()
            .find(|c| c.info.name == name)
            .ok_or_else(|| HalError::Invalid(format!("no mixer control {name:?}")))
    }
}

impl Mixer for AlsaMixer {
    fn controls(&mut self) -> Result<Vec<ControlInfo>> {
        Ok(self.ctls.iter().map(|c| c.info.clone()).collect())
    }

    fn get(&mut self, name: &str) -> Result<Vec<i32>> {
        let c = self.find(name)?;
        // SAFETY: plain data; zero is valid.
        let mut v: Box<SndCtlElemValue> = Box::new(unsafe { std::mem::zeroed() });
        v.id = c.id;
        ioctl(self.file.as_raw_fd(), SNDRV_CTL_IOCTL_ELEM_READ, &mut *v)
            .map_err(|e| HalError::io(format!("read control {name:?}"), e))?;
        let n = (c.info.count as usize).min(64);
        // SAFETY: reading the union member that matches the control type.
        Ok(match c.type_ {
            ELEM_TYPE_ENUMERATED => unsafe { v.value.enumerated[..n].iter().map(|x| *x as i32).collect() },
            _ => unsafe { v.value.integer[..n].iter().map(|x| *x as i32).collect() },
        })
    }

    fn set(&mut self, name: &str, values: &[i32]) -> Result<()> {
        let c = self.find(name)?;
        if values.is_empty() {
            return Err(HalError::Invalid("no values".into()));
        }
        if !matches!(c.info.kind, ControlKind::Boolean | ControlKind::Integer | ControlKind::Enumerated) {
            return Err(HalError::Unsupported("control type"));
        }
        // SAFETY: plain data; zero is valid.
        let mut v: Box<SndCtlElemValue> = Box::new(unsafe { std::mem::zeroed() });
        v.id = c.id;
        for i in 0..(c.info.count as usize).min(64) {
            let x = values.get(i).copied().unwrap_or(values[0]).clamp(c.info.min, c.info.max);
            // SAFETY: writing the union member that matches the type.
            unsafe {
                if c.type_ == ELEM_TYPE_ENUMERATED {
                    v.value.enumerated[i] = x as u32;
                } else {
                    v.value.integer[i] = x as c_long;
                }
            }
        }
        ioctl(self.file.as_raw_fd(), SNDRV_CTL_IOCTL_ELEM_WRITE, &mut *v)
            .map_err(|e| HalError::io(format!("write control {name:?}"), e))?;
        Ok(())
    }
}

/// The phone's sound card.
pub struct AlsaAudio {
    card: u32,
    rate: u32,
    open_set: Arc<Mutex<[bool; 3]>>,
    /// The D80's card: one stereo PCM device, and a control for the path.
    one_device: bool,
}

/// The control of the D80's card that selects the path.  Its items are in
/// the order of [`PcmDevice::index`]: `Speaker`, `Handset`, `Headset` (and
/// then `Bluetooth`).
pub const D80_PATH_CONTROL: &str = "Audio Path";
/// The item of [`D80_PATH_CONTROL`] for a Bluetooth headset.  On it the
/// voice processor exchanges the call's audio with the Bluetooth module by
/// itself, over its other audio port, and the SoC's side of the call stays
/// on the one PCM device, at its own rate.
pub const D80_PATH_BLUETOOTH: i32 = 3;
/// The control of the D80's card for the rate of the audio between the
/// voice processor and the Bluetooth module.  Its items are `8000`, for a
/// narrow-band voice link, and `16000`, for wide-band speech.
pub const D80_BLUETOOTH_RATE_CONTROL: &str = "Bluetooth Sample Rate";
/// The control of the D80's card for the voice processor's output level,
/// 0 (silence, which it starts at) to 10.
pub const D80_VOLUME_CONTROL: &str = "Master Playback Volume";
/// The level the D80's voice processor is left at.  The volume the user
/// sets is applied to the samples before they reach it, as the DSP of the
/// other models applies it, so this is a fixed level: the highest that
/// stock does not add a further speaker gain at (it does at 8, 9, and 10).
/// How loud that is has not been measured.
pub const D80_VOLUME: i32 = 7;

impl AlsaAudio {
    pub fn open(model: Model) -> Result<AlsaAudio> {
        Ok(AlsaAudio { card: find_card()?, rate: model.audio_rate(), open_set: Default::default(), one_device: model.is_touch() })
    }
}

impl AlsaAudio {
    /// Opens the streams of PCM device `number`, with `channels` samples a
    /// frame, for the path `device`, which is marked busy while they are
    /// open.
    fn open_streams(&mut self, device: PcmDevice, number: u32, channels: usize, config: PcmConfig) -> Result<Box<dyn Pcm>> {
        if config.period_frames == 0 || !(config.capture || config.playback) {
            return Err(HalError::Invalid("empty PCM configuration".into()));
        }
        {
            let mut set = self.open_set.lock().unwrap();
            for other in [PcmDevice::Speaker, PcmDevice::Handset, PcmDevice::Headset] {
                // The D80's one device carries one path at a time.
                if set[other.index() as usize] && (other == device || other.conflicts_with(device) || self.one_device) {
                    return Err(HalError::Invalid(format!("{device:?} is busy ({other:?} is open)")));
                }
            }
            set[device.index() as usize] = true;
        }
        let mut pcm = AlsaPcm { device, config, capture: None, playback: None, open_set: self.open_set.clone(), stereo: Vec::new() };
        // On error `pcm` drops and clears the busy flag.
        if config.playback {
            let (s, period, periods) = Stream::open(self.card, number, channels, true, self.rate, &config)?;
            pcm.config.period_frames = period;
            pcm.config.periods = periods;
            pcm.playback = Some(s);
        }
        if config.capture {
            let (s, period, periods) = Stream::open(self.card, number, channels, false, self.rate, &config)?;
            pcm.config.period_frames = period;
            pcm.config.periods = periods;
            pcm.capture = Some(s);
        }
        Ok(Box::new(pcm))
    }

    /// The D80: selects the item `path` of [`D80_PATH_CONTROL`] and opens
    /// the one stereo device.  `device` is the path it counts as open for.
    /// `wideband` is the Bluetooth path's rate.
    fn open_one_device(&mut self, device: PcmDevice, path: i32, wideband: bool, config: PcmConfig) -> Result<Box<dyn Pcm>> {
        // The voice processor changes path in steps, over up to about a
        // second, after the control is written; the streams can be opened
        // meanwhile.
        let mut mixer = AlsaMixer::open(self.card)?;
        if path == D80_PATH_BLUETOOTH {
            // Before the path, which is loaded with the rate.
            mixer.set(D80_BLUETOOTH_RATE_CONTROL, &[i32::from(wideband)])?;
        }
        mixer.set(D80_PATH_CONTROL, &[path])?;
        mixer.set(D80_VOLUME_CONTROL, &[D80_VOLUME])?;
        self.open_streams(device, 0, 2, config)
    }
}

impl Audio for AlsaAudio {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn open(&mut self, device: PcmDevice, config: PcmConfig) -> Result<Box<dyn Pcm>> {
        if self.one_device {
            return self.open_one_device(device, device.index() as i32, false, config);
        }
        self.open_streams(device, device.index(), 1, config)
    }

    fn mixer(&mut self) -> Result<Box<dyn Mixer>> {
        Ok(Box::new(AlsaMixer::open(self.card)?))
    }

    fn open_bluetooth(&mut self, config: PcmConfig, wideband: bool) -> Result<Box<dyn Pcm>> {
        if self.one_device {
            return self.open_one_device(PcmDevice::Headset, D80_PATH_BLUETOOTH, wideband, config);
        }
        let card = find_card_named(crate::BLUETOOTH_CARD)
            .ok_or_else(|| HalError::Unavailable(format!("no ALSA card named {}", crate::BLUETOOTH_CARD)))?;
        // The card has one device, number 0 (the number `Speaker` has on
        // the phone's own card), and its own busy flags.
        let device = PcmDevice::Speaker;
        let mut pcm = AlsaPcm { device, config, capture: None, playback: None, open_set: Default::default(), stereo: Vec::new() };
        if config.playback {
            let (s, period, periods) = Stream::open(card, device.index(), 1, true, crate::BLUETOOTH_RATE, &config)?;
            pcm.config.period_frames = period;
            pcm.config.periods = periods;
            pcm.playback = Some(s);
        }
        if config.capture {
            let (s, period, periods) = Stream::open(card, device.index(), 1, false, crate::BLUETOOTH_RATE, &config)?;
            pcm.config.period_frames = period;
            pcm.config.periods = periods;
            pcm.capture = Some(s);
        }
        Ok(Box::new(pcm))
    }
}

// SAFETY: `SndCtlElemList` holds a raw pointer only transiently, and none
// of these types is shared; the trait objects need `Send`.
unsafe impl Send for AlsaMixer {}

#[cfg(test)]
mod tests {
    use super::{mono_to_stereo, playback_start_threshold, stereo_to_mono};

    /// The D80's device is stereo with the voice in the left channel.
    #[test]
    fn mono_samples_go_through_the_stereo_device() {
        let mut stereo = vec![9; 3];
        mono_to_stereo(&[1, -2, 3], &mut stereo);
        assert_eq!(stereo, [1, 1, -2, -2, 3, 3]);
        let mut mono = [0i16; 3];
        stereo_to_mono(&[5, 100, 6, 200, 7, 300], &mut mono);
        assert_eq!(mono, [5, 6, 7]);
    }

    #[test]
    fn playback_starts_with_half_the_buffer_queued() {
        // The phone's voice streams: 5 ms periods, 8 of them, at 32 kHz
        // and at 16 kHz: two 10 ms writes start playback.
        assert_eq!(playback_start_threshold(160, 1280), 640);
        assert_eq!(playback_start_threshold(80, 640), 320);
        // A buffer the driver made smaller or odd: whole periods, never
        // more than the buffer, never zero.
        assert_eq!(playback_start_threshold(160, 640), 320);
        assert_eq!(playback_start_threshold(160, 320), 160);
        assert_eq!(playback_start_threshold(160, 160), 160);
        assert_eq!(playback_start_threshold(100, 550), 200);
        assert_eq!(playback_start_threshold(0, 0), 1);
    }
}
