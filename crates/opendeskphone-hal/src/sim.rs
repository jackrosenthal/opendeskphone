//! The simulator backend: hardware in memory, for workstations and tests.
//!
//! [`crate::Hal::sim`] returns a bundle whose `sim` field is a
//! [`SimHandle`]: the "outside" of the simulated phone.  Through it a test
//! or the web screen mirror reads framebuffers and lamps, presses keys,
//! lifts the handset, feeds and collects audio, and inspects flash.

use crate::*;
use std::collections::{BTreeMap, VecDeque};
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// NAND geometry of the phones.
pub const SIM_ERASE_SIZE: u32 = 128 * 1024;
pub const SIM_PAGE_SIZE: u32 = 2048;
pub const SIM_OOB_SIZE: u32 = 64;

/// The stock partition layout (as boot2 passes it on a D60).
pub const SIM_PARTITIONS: [(&str, u64); 9] = [
    ("boot1", 0x0002_0000),
    ("boot2", 0x0008_0000),
    ("ctrl", 0x0008_0000),
    ("nvdata", 0x00ee_0000),
    ("k0", 0x0020_0000),
    ("k1", 0x0020_0000),
    ("root0", 0x02d0_0000),
    ("root1", 0x02d0_0000),
    ("userdata", 0x0100_0000),
];

struct SimScreen {
    screen: Screen,
    frame: Frame,
    power: bool,
    brightness: u8,
    contrast: u8,
    /// What the panel showed when it was first turned on.
    first_lit: Option<Frame>,
}

/// Something done to a simulated display or its backlight, in the order it
/// happened (see [`SimHandle::display_events`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimDisplayEvent {
    Contrast(Screen, u8),
    Brightness(Screen, u8),
    Flush(Screen),
    Power(Screen, bool),
}

/// State of the simulated watchdog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimWatchdog {
    pub armed: bool,
    pub timeout_secs: u32,
    pub feeds: u64,
    /// Milliseconds since the last feed (or start), when armed.
    pub since_feed_ms: u64,
}

struct WdInner {
    armed: bool,
    timeout: u32,
    feeds: u64,
    last: Instant,
}

struct SimPart {
    info: MtdInfo,
    /// `None` until first written: reads as erased (0xFF).
    data: Option<Vec<u8>>,
    bad: Vec<bool>,
    erase_counts: Vec<u32>,
}

#[derive(Default)]
struct SimPcmState {
    open: bool,
    /// Samples written to playback, for the test side to collect.
    played: VecDeque<i16>,
    /// Samples the test side queued for capture.
    capture: VecDeque<i16>,
}

struct SimAudioState {
    devices: [SimPcmState; 3],
    /// Captured audio is what was just played (after the queue runs dry).
    loopback: bool,
    /// Reads take real time (one period per period).
    realtime: bool,
    controls: Vec<(ControlInfo, Vec<i32>)>,
}

/// Cap on audio kept for collection (10 s at 32 kHz).
const SIM_PLAYED_CAP: usize = 320_000;

struct Shared {
    model: Model,
    screens: Mutex<Vec<SimScreen>>,
    display_events: Mutex<Vec<SimDisplayEvent>>,
    frame_gen: AtomicU64,
    input: Mutex<VecDeque<InputEvent>>,
    input_cv: Condvar,
    off_hook: AtomicBool,
    lamps: Mutex<BTreeMap<Lamp, LampColor>>,
    watchdog: Mutex<WdInner>,
    mtd: Mutex<Vec<SimPart>>,
    audio: Mutex<SimAudioState>,
    ehs_rx: Mutex<VecDeque<u8>>,
    ehs_rx_cv: Condvar,
    ehs_tx: Mutex<Vec<u8>>,
    ehs_open: AtomicBool,
}

/// The outside of the simulated phone.  Clonable; all clones share state.
#[derive(Clone)]
pub struct SimHandle(Arc<Shared>);

/// Builds the simulator HAL; see [`crate::Hal::sim`].
pub fn build(model: Model) -> Hal {
    let handle = SimHandle::new(model);
    Hal {
        model,
        display: Box::new(handle.display()),
        backlight: Box::new(SimDev(handle.clone())),
        input: Box::new(SimDev(handle.clone())),
        lamps: Box::new(SimDev(handle.clone())),
        audio: Box::new(SimDev(handle.clone())),
        mtd: Box::new(SimDev(handle.clone())),
        watchdog: Box::new(SimDev(handle.clone())),
        rng: Box::new(SimRng::new(0x9e37_79b9_7f4a_7c15)),
        ehs: Box::new(SimDev(handle.clone())),
        sim: Some(handle),
        missing: Vec::new(),
    }
}

impl SimHandle {
    /// A fresh simulated phone.  Normally obtained through
    /// [`crate::Hal::sim`]; more device objects for the same phone can be
    /// made with [`SimHandle::display`] and [`SimHandle::devices`].
    pub fn new(model: Model) -> SimHandle {
        let screens = [Screen::Main, Screen::Side]
            .into_iter()
            .filter_map(|s| {
                let info = FrameInfo::for_model(model, s)?;
                Some(SimScreen {
                    screen: s,
                    frame: Frame { info, pixels: vec![0; info.len()] },
                    // As the kernel leaves a phone: the panel off (no mode
                    // set), and the backlight level at the driver's default.
                    power: false,
                    brightness: model.backlight_max(),
                    contrast: 5,
                    first_lit: None,
                })
            })
            .collect();
        let mtd = SIM_PARTITIONS
            .iter()
            .enumerate()
            .map(|(i, (name, size))| {
                let blocks = (*size / SIM_ERASE_SIZE as u64) as usize;
                SimPart {
                    info: MtdInfo {
                        name: name.to_string(),
                        index: i as u32,
                        size: *size,
                        erase_size: SIM_ERASE_SIZE,
                        write_size: SIM_PAGE_SIZE,
                        oob_size: SIM_OOB_SIZE,
                    },
                    data: None,
                    bad: vec![false; blocks],
                    erase_counts: vec![0; blocks],
                }
            })
            .collect();
        let int = |name: &str, max: i32, value: i32| {
            (
                ControlInfo {
                    name: name.to_string(),
                    kind: ControlKind::Integer,
                    count: 1,
                    min: 0,
                    max,
                    items: Vec::new(),
                },
                vec![value],
            )
        };
        SimHandle(Arc::new(Shared {
            model,
            screens: Mutex::new(screens),
            display_events: Mutex::new(Vec::new()),
            frame_gen: AtomicU64::new(0),
            input: Mutex::new(VecDeque::new()),
            input_cv: Condvar::new(),
            off_hook: AtomicBool::new(false),
            lamps: Mutex::new(Lamp::for_model(model).into_iter().map(|l| (l, LampColor::Off)).collect()),
            watchdog: Mutex::new(WdInner { armed: false, timeout: 0, feeds: 0, last: Instant::now() }),
            mtd: Mutex::new(mtd),
            audio: Mutex::new(SimAudioState {
                devices: Default::default(),
                loopback: false,
                realtime: true,
                controls: vec![int("Sim Playback Volume", 100, 50), int("Sim Capture Volume", 100, 50)],
            }),
            ehs_rx: Mutex::new(VecDeque::new()),
            ehs_rx_cv: Condvar::new(),
            ehs_tx: Mutex::new(Vec::new()),
            ehs_open: AtomicBool::new(false),
        }))
    }

    pub fn model(&self) -> Model {
        self.0.model
    }

    /// A display device for this phone (each has its own back buffer).
    pub fn display(&self) -> SimDisplay {
        let back = self
            .0
            .screens
            .lock()
            .unwrap()
            .iter()
            .map(|s| (s.screen, s.frame.info, s.frame.pixels.clone()))
            .collect();
        SimDisplay { sim: self.clone(), back }
    }

    /// A device object implementing every other HAL trait for this phone.
    pub fn devices(&self) -> SimDev {
        SimDev(self.clone())
    }

    // ---- display side

    /// What the screen shows (the last flushed frame).
    pub fn frame(&self, screen: Screen) -> Option<Frame> {
        self.0.screens.lock().unwrap().iter().find(|s| s.screen == screen).map(|s| s.frame.clone())
    }

    /// Increases on every flush; lets a mirror skip unchanged frames.
    pub fn frame_generation(&self) -> u64 {
        self.0.frame_gen.load(Ordering::Relaxed)
    }

    pub fn screen_power(&self, screen: Screen) -> Option<bool> {
        self.0.screens.lock().unwrap().iter().find(|s| s.screen == screen).map(|s| s.power)
    }

    /// What the panel showed at the moment it was first turned on.
    pub fn first_lit(&self, screen: Screen) -> Option<Frame> {
        self.0.screens.lock().unwrap().iter().find(|s| s.screen == screen).and_then(|s| s.first_lit.clone())
    }

    /// Every flush, panel power change, and backlight or contrast level
    /// set so far, in order.
    pub fn display_events(&self) -> Vec<SimDisplayEvent> {
        self.0.display_events.lock().unwrap().clone()
    }

    fn display_event(&self, event: SimDisplayEvent) {
        self.0.display_events.lock().unwrap().push(event);
    }

    pub fn brightness(&self, screen: Screen) -> Option<u8> {
        self.0.screens.lock().unwrap().iter().find(|s| s.screen == screen).map(|s| s.brightness)
    }

    pub fn contrast(&self, screen: Screen) -> Option<u8> {
        self.0.screens.lock().unwrap().iter().find(|s| s.screen == screen).map(|s| s.contrast)
    }

    // ---- input side

    /// Queues an input event, as if from the keypad.
    pub fn inject(&self, event: InputEvent) {
        if let InputEvent::Hook { off_hook } = event {
            self.0.off_hook.store(off_hook, Ordering::SeqCst);
        }
        self.0.input.lock().unwrap().push_back(event);
        self.0.input_cv.notify_all();
    }

    pub fn key(&self, key: Key, down: bool) {
        self.inject(InputEvent::Key { key, down });
    }

    /// Press and release.
    pub fn tap(&self, key: Key) {
        self.key(key, true);
        self.key(key, false);
    }

    /// Lifts (`true`) or replaces the handset.
    pub fn set_hook(&self, off_hook: bool) {
        self.inject(InputEvent::Hook { off_hook });
    }

    /// One event of a touch of the main screen (D80).
    pub fn touch(&self, phase: TouchPhase, x: u16, y: u16) {
        self.inject(InputEvent::Touch { phase, x, y });
    }

    /// A finger put down at (`x`, `y`) and lifted there.
    pub fn touch_tap(&self, x: u16, y: u16) {
        self.touch(TouchPhase::Down, x, y);
        self.touch(TouchPhase::Up, x, y);
    }

    /// A finger put down at `from`, moved to `to` in `steps` moves, and
    /// lifted there.
    pub fn touch_drag(&self, from: (u16, u16), to: (u16, u16), steps: u16) {
        self.touch(TouchPhase::Down, from.0, from.1);
        let at = |a: u16, b: u16, i: u16| (a as i32 + (b as i32 - a as i32) * i as i32 / steps.max(1) as i32) as u16;
        for i in 1..=steps.max(1) {
            self.touch(TouchPhase::Move, at(from.0, to.0, i), at(from.1, to.1, i));
        }
        self.touch(TouchPhase::Up, to.0, to.1);
    }

    // ---- lamps

    pub fn lamp(&self, lamp: Lamp) -> LampColor {
        self.0.lamps.lock().unwrap().get(&lamp).copied().unwrap_or(LampColor::Off)
    }

    pub fn lamps(&self) -> Vec<(Lamp, LampColor)> {
        self.0.lamps.lock().unwrap().iter().map(|(l, c)| (*l, *c)).collect()
    }

    // ---- watchdog

    pub fn watchdog(&self) -> SimWatchdog {
        let g = self.0.watchdog.lock().unwrap();
        SimWatchdog {
            armed: g.armed,
            timeout_secs: g.timeout,
            feeds: g.feeds,
            since_feed_ms: g.last.elapsed().as_millis() as u64,
        }
    }

    // ---- flash

    /// The whole contents of a partition.
    pub fn mtd_contents(&self, name: &str) -> Option<Vec<u8>> {
        let g = self.0.mtd.lock().unwrap();
        let p = g.iter().find(|p| p.info.name == name)?;
        Some(p.data.clone().unwrap_or_else(|| vec![0xff; p.info.size as usize]))
    }

    /// Loads data at the start of a partition (rest erased), bypassing
    /// erase/write rules.
    pub fn mtd_load(&self, name: &str, data: &[u8]) -> Result<()> {
        let mut g = self.0.mtd.lock().unwrap();
        let p = g
            .iter_mut()
            .find(|p| p.info.name == name)
            .ok_or_else(|| HalError::Invalid(format!("no partition {name}")))?;
        if data.len() as u64 > p.info.size {
            return Err(HalError::Invalid(format!("{} bytes do not fit in {name}", data.len())));
        }
        let mut v = vec![0xff; p.info.size as usize];
        v[..data.len()].copy_from_slice(data);
        p.data = Some(v);
        Ok(())
    }

    /// Loads a partition image from a file.
    pub fn mtd_load_file(&self, name: &str, path: &Path) -> Result<()> {
        let data = std::fs::read(path).map_err(|e| HalError::io(path.display().to_string(), e))?;
        self.mtd_load(name, &data)
    }

    /// Saves a partition's contents to a file.
    pub fn mtd_save_file(&self, name: &str, path: &Path) -> Result<()> {
        let data = self
            .mtd_contents(name)
            .ok_or_else(|| HalError::Invalid(format!("no partition {name}")))?;
        std::fs::write(path, data).map_err(|e| HalError::io(path.display().to_string(), e))
    }

    /// Marks an erase block (by index) bad or good.
    pub fn mtd_set_bad(&self, name: &str, block: usize, bad: bool) {
        let mut g = self.0.mtd.lock().unwrap();
        if let Some(b) = g.iter_mut().find(|p| p.info.name == name).and_then(|p| p.bad.get_mut(block)) {
            *b = bad;
        }
    }

    /// How many times an erase block has been erased.
    pub fn mtd_erase_count(&self, name: &str, block: usize) -> u32 {
        let g = self.0.mtd.lock().unwrap();
        g.iter()
            .find(|p| p.info.name == name)
            .and_then(|p| p.erase_counts.get(block).copied())
            .unwrap_or(0)
    }

    // ---- audio

    /// Capture returns what was played (an acoustic loop), once any queued
    /// capture samples are used up.
    pub fn audio_set_loopback(&self, on: bool) {
        self.0.audio.lock().unwrap().loopback = on;
    }

    /// Whether `read` takes real time (default) or returns at once (tests).
    pub fn audio_set_realtime(&self, on: bool) {
        self.0.audio.lock().unwrap().realtime = on;
    }

    /// Queues samples for a device's capture direction (a "talker").
    pub fn audio_feed(&self, device: PcmDevice, samples: &[i16]) {
        self.0.audio.lock().unwrap().devices[device.index() as usize].capture.extend(samples);
    }

    /// Takes everything played on a device since the last call.
    pub fn audio_played(&self, device: PcmDevice) -> Vec<i16> {
        self.0.audio.lock().unwrap().devices[device.index() as usize].played.drain(..).collect()
    }

    pub fn audio_is_open(&self, device: PcmDevice) -> bool {
        self.0.audio.lock().unwrap().devices[device.index() as usize].open
    }

    // ---- EHS

    /// Bytes "from the adapter" for the phone to read.
    pub fn ehs_feed(&self, data: &[u8]) {
        self.0.ehs_rx.lock().unwrap().extend(data);
        self.0.ehs_rx_cv.notify_all();
    }

    /// Takes the bytes the phone wrote to the adapter.
    pub fn ehs_written(&self) -> Vec<u8> {
        std::mem::take(&mut self.0.ehs_tx.lock().unwrap())
    }
}

/// The simulated display device.
pub struct SimDisplay {
    sim: SimHandle,
    back: Vec<(Screen, FrameInfo, Vec<u8>)>,
}

impl Display for SimDisplay {
    fn screens(&self) -> Vec<Screen> {
        self.back.iter().map(|b| b.0).collect()
    }

    fn info(&self, screen: Screen) -> Option<FrameInfo> {
        self.back.iter().find(|b| b.0 == screen).map(|b| b.1)
    }

    fn frame(&mut self, screen: Screen) -> Result<&mut [u8]> {
        self.back
            .iter_mut()
            .find(|b| b.0 == screen)
            .map(|b| b.2.as_mut_slice())
            .ok_or(HalError::Unsupported("no such screen on this model"))
    }

    fn flush(&mut self, screen: Screen, damage: &[Rect]) -> Result<()> {
        let (_, info, back) = self
            .back
            .iter()
            .find(|b| b.0 == screen)
            .ok_or(HalError::Unsupported("no such screen on this model"))?;
        let mut g = self.sim.0.screens.lock().unwrap();
        let front = g.iter_mut().find(|s| s.screen == screen).expect("screen exists");
        if damage.is_empty() {
            front.frame.pixels.copy_from_slice(back);
        } else {
            let bpp = info.format.bytes_per_pixel();
            for r in damage {
                let r = r.clip(info.width, info.height);
                for y in r.y..r.y + r.height {
                    let a = y as usize * info.stride + r.x as usize * bpp;
                    let b = a + r.width as usize * bpp;
                    front.frame.pixels[a..b].copy_from_slice(&back[a..b]);
                }
            }
        }
        drop(g);
        self.sim.0.frame_gen.fetch_add(1, Ordering::Relaxed);
        self.sim.display_event(SimDisplayEvent::Flush(screen));
        Ok(())
    }

    fn capture(&self, screen: Screen) -> Result<Frame> {
        self.sim.frame(screen).ok_or(HalError::Unsupported("no such screen on this model"))
    }

    fn set_power(&mut self, screen: Screen, on: bool) -> Result<()> {
        let mut g = self.sim.0.screens.lock().unwrap();
        let s = g
            .iter_mut()
            .find(|s| s.screen == screen)
            .ok_or(HalError::Unsupported("no such screen on this model"))?;
        if on && !s.power {
            // Turning a panel on shows the whole buffer as it is now.
            if let Some((_, _, back)) = self.back.iter().find(|b| b.0 == screen) {
                s.frame.pixels.copy_from_slice(back);
            }
            if s.first_lit.is_none() {
                s.first_lit = Some(s.frame.clone());
            }
        }
        s.power = on;
        drop(g);
        self.sim.display_event(SimDisplayEvent::Power(screen, on));
        Ok(())
    }
}

/// Implements every HAL trait except [`Display`] and [`Rng`] against a
/// [`SimHandle`].
pub struct SimDev(SimHandle);

impl SimDev {
    fn with_screen<R>(&self, screen: Screen, f: impl FnOnce(&mut SimScreen) -> R) -> Result<R> {
        let mut g = self.0 .0.screens.lock().unwrap();
        g.iter_mut()
            .find(|s| s.screen == screen)
            .map(f)
            .ok_or(HalError::Unsupported("no such screen on this model"))
    }
}

impl Backlight for SimDev {
    fn max_brightness(&self, _: Screen) -> u8 {
        self.0.model().backlight_max()
    }
    fn brightness(&self, screen: Screen) -> Result<u8> {
        self.with_screen(screen, |s| s.brightness)
    }
    fn set_brightness(&mut self, screen: Screen, level: u8) -> Result<()> {
        let max = self.max_brightness(screen);
        self.with_screen(screen, |s| s.brightness = level.min(max))?;
        self.0.display_event(SimDisplayEvent::Brightness(screen, level.min(max)));
        Ok(())
    }
    fn max_contrast(&self, _: Screen) -> u8 {
        if self.0.model().is_color() { 0 } else { 10 }
    }
    fn contrast(&self, screen: Screen) -> Result<u8> {
        self.with_screen(screen, |s| s.contrast)
    }
    fn set_contrast(&mut self, screen: Screen, level: u8) -> Result<()> {
        let max = self.max_contrast(screen);
        if max == 0 {
            return Err(HalError::Unsupported("the color panel has no contrast control"));
        }
        self.with_screen(screen, |s| s.contrast = level.min(max))?;
        self.0.display_event(SimDisplayEvent::Contrast(screen, level.min(max)));
        Ok(())
    }
}

impl Input for SimDev {
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<InputEvent>> {
        let sh = &self.0 .0;
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut g = sh.input.lock().unwrap();
        loop {
            if let Some(e) = g.pop_front() {
                return Ok(Some(e));
            }
            match deadline {
                None => g = sh.input_cv.wait(g).unwrap(),
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return Ok(None);
                    }
                    g = sh.input_cv.wait_timeout(g, d - now).unwrap().0;
                }
            }
        }
    }
    fn off_hook(&mut self) -> Result<bool> {
        Ok(self.0 .0.off_hook.load(Ordering::SeqCst))
    }
    fn poll_fds(&self) -> Vec<RawFd> {
        Vec::new()
    }
}

impl Lamps for SimDev {
    fn lamps(&self) -> Vec<Lamp> {
        self.0 .0.lamps.lock().unwrap().keys().copied().collect()
    }
    fn set(&mut self, lamp: Lamp, color: LampColor) -> Result<()> {
        if let Some(c) = self.0 .0.lamps.lock().unwrap().get_mut(&lamp) {
            *c = color;
        }
        Ok(())
    }
}

impl Watchdog for SimDev {
    fn start(&mut self, timeout_secs: u32) -> Result<u32> {
        let mut g = self.0 .0.watchdog.lock().unwrap();
        g.armed = true;
        g.timeout = timeout_secs.clamp(1, 60);
        g.last = Instant::now();
        Ok(g.timeout)
    }
    fn feed(&mut self) -> Result<()> {
        let mut g = self.0 .0.watchdog.lock().unwrap();
        if !g.armed {
            return Err(HalError::Invalid("watchdog not started".into()));
        }
        g.feeds += 1;
        g.last = Instant::now();
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.0 .0.watchdog.lock().unwrap().armed = false;
        Ok(())
    }
    fn time_left(&mut self) -> Result<u32> {
        let g = self.0 .0.watchdog.lock().unwrap();
        if !g.armed {
            return Err(HalError::Invalid("watchdog not started".into()));
        }
        Ok(g.timeout.saturating_sub(g.last.elapsed().as_secs() as u32))
    }
}

/// Deterministic pseudo-random bytes (xorshift64*); not for real keys.
pub struct SimRng(u64);

impl SimRng {
    pub fn new(seed: u64) -> Self {
        SimRng(seed | 1)
    }
}

impl Rng for SimRng {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        for chunk in buf.chunks_mut(8) {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            let v = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ MTD

impl Mtd for SimDev {
    fn partitions(&self) -> Result<Vec<MtdInfo>> {
        Ok(self.0 .0.mtd.lock().unwrap().iter().map(|p| p.info.clone()).collect())
    }
    fn open(&mut self, name: &str) -> Result<Box<dyn MtdPartition>> {
        let g = self.0 .0.mtd.lock().unwrap();
        let (idx, p) = g
            .iter()
            .enumerate()
            .find(|(_, p)| p.info.name == name)
            .ok_or_else(|| HalError::Unavailable(format!("no MTD partition named {name}")))?;
        Ok(Box::new(SimPartition { sim: self.0.clone(), idx, info: p.info.clone() }))
    }
}

struct SimPartition {
    sim: SimHandle,
    idx: usize,
    info: MtdInfo,
}

impl SimPartition {
    fn check(&self, offset: u64, len: usize) -> Result<()> {
        if offset.checked_add(len as u64).is_none_or(|end| end > self.info.size) {
            return Err(HalError::Invalid(format!(
                "{:#x}+{len:#x} is outside {} ({:#x} bytes)",
                offset, self.info.name, self.info.size
            )));
        }
        Ok(())
    }
}

impl MtdPartition for SimPartition {
    fn info(&self) -> &MtdInfo {
        &self.info
    }

    fn is_bad(&mut self, offset: u64) -> Result<bool> {
        self.check(offset, 0)?;
        let g = self.sim.0.mtd.lock().unwrap();
        Ok(g[self.idx].bad.get((offset / self.info.erase_size as u64) as usize).copied().unwrap_or(false))
    }

    fn erase(&mut self, offset: u64) -> Result<()> {
        let bs = self.info.erase_size as u64;
        if !offset.is_multiple_of(bs) {
            return Err(HalError::Invalid(format!("erase offset {offset:#x} not block aligned")));
        }
        self.check(offset, bs as usize)?;
        let mut g = self.sim.0.mtd.lock().unwrap();
        let p = &mut g[self.idx];
        let block = (offset / bs) as usize;
        if p.bad[block] {
            return Err(HalError::BadBlock(offset));
        }
        p.erase_counts[block] += 1;
        if let Some(d) = &mut p.data {
            d[offset as usize..(offset + bs) as usize].fill(0xff);
        }
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.check(offset, buf.len())?;
        let g = self.sim.0.mtd.lock().unwrap();
        match &g[self.idx].data {
            Some(d) => buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]),
            None => buf.fill(0xff),
        }
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        if !offset.is_multiple_of(self.info.write_size as u64) {
            return Err(HalError::Invalid(format!("write offset {offset:#x} not page aligned")));
        }
        self.check(offset, data.len())?;
        let bs = self.info.erase_size as u64;
        let mut g = self.sim.0.mtd.lock().unwrap();
        let p = &mut g[self.idx];
        if !data.is_empty() {
            let first = (offset / bs) as usize;
            let last = ((offset + data.len() as u64 - 1) / bs) as usize;
            if let Some(b) = (first..=last).find(|b| p.bad[*b]) {
                return Err(HalError::BadBlock(b as u64 * bs));
            }
        }
        let size = p.info.size as usize;
        let d = p.data.get_or_insert_with(|| vec![0xff; size]);
        // NAND programming can only clear bits.
        for (dst, src) in d[offset as usize..].iter_mut().zip(data) {
            *dst &= *src;
        }
        Ok(())
    }

    fn mark_bad(&mut self, offset: u64) -> Result<()> {
        self.check(offset, 0)?;
        let mut g = self.sim.0.mtd.lock().unwrap();
        let block = (offset / self.info.erase_size as u64) as usize;
        if let Some(b) = g[self.idx].bad.get_mut(block) {
            *b = true;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- audio

impl Audio for SimDev {
    fn rate(&self) -> u32 {
        self.0.model().audio_rate()
    }

    fn open(&mut self, device: PcmDevice, config: PcmConfig) -> Result<Box<dyn Pcm>> {
        if config.period_frames == 0 || !(config.capture || config.playback) {
            return Err(HalError::Invalid("empty PCM configuration".into()));
        }
        let mut g = self.0 .0.audio.lock().unwrap();
        for other in [PcmDevice::Speaker, PcmDevice::Handset, PcmDevice::Headset] {
            let o = &g.devices[other.index() as usize];
            if o.open && (other == device || other.conflicts_with(device)) {
                return Err(HalError::Invalid(format!("{device:?} is busy ({other:?} is open)")));
            }
        }
        let d = &mut g.devices[device.index() as usize];
        d.open = true;
        d.played.clear();
        Ok(Box::new(SimPcm {
            sim: self.0.clone(),
            device,
            config,
            next: Instant::now(),
            rate: self.0.model().audio_rate(),
        }))
    }

    fn mixer(&mut self) -> Result<Box<dyn Mixer>> {
        Ok(Box::new(SimDev(self.0.clone())))
    }
}

struct SimPcm {
    sim: SimHandle,
    device: PcmDevice,
    config: PcmConfig,
    next: Instant,
    rate: u32,
}

impl Pcm for SimPcm {
    fn config(&self) -> PcmConfig {
        self.config
    }

    fn read(&mut self, buf: &mut [i16]) -> Result<()> {
        if !self.config.capture {
            return Err(HalError::Invalid("PCM not opened for capture".into()));
        }
        let realtime = self.sim.0.audio.lock().unwrap().realtime;
        if realtime {
            // Pace like hardware: this many samples take this long.
            let dur = Duration::from_micros(buf.len() as u64 * 1_000_000 / self.rate as u64);
            let now = Instant::now();
            if self.next + Duration::from_millis(500) < now {
                self.next = now; // fell far behind: resynchronize
            }
            self.next += dur;
            if let Some(wait) = self.next.checked_duration_since(now) {
                std::thread::sleep(wait);
            }
        }
        let mut g = self.sim.0.audio.lock().unwrap();
        let loopback = g.loopback;
        let d = &mut g.devices[self.device.index() as usize];
        for s in buf.iter_mut() {
            *s = match d.capture.pop_front() {
                Some(v) => v,
                None if loopback => d.played.pop_front().unwrap_or(0),
                None => 0,
            };
        }
        Ok(())
    }

    fn write(&mut self, buf: &[i16]) -> Result<()> {
        if !self.config.playback {
            return Err(HalError::Invalid("PCM not opened for playback".into()));
        }
        let mut g = self.sim.0.audio.lock().unwrap();
        let d = &mut g.devices[self.device.index() as usize];
        d.played.extend(buf);
        while d.played.len() > SIM_PLAYED_CAP {
            d.played.pop_front();
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<()> {
        self.next = Instant::now();
        Ok(())
    }

    fn playback_delay(&mut self) -> Result<u32> {
        Ok(0)
    }
}

impl Drop for SimPcm {
    fn drop(&mut self) {
        self.sim.0.audio.lock().unwrap().devices[self.device.index() as usize].open = false;
    }
}

impl Mixer for SimDev {
    fn controls(&mut self) -> Result<Vec<ControlInfo>> {
        Ok(self.0 .0.audio.lock().unwrap().controls.iter().map(|c| c.0.clone()).collect())
    }

    fn get(&mut self, name: &str) -> Result<Vec<i32>> {
        self.0 .0.audio.lock().unwrap().controls.iter().find(|c| c.0.name == name).map(|c| c.1.clone())
            .ok_or_else(|| HalError::Invalid(format!("no mixer control {name:?}")))
    }

    fn set(&mut self, name: &str, values: &[i32]) -> Result<()> {
        let mut g = self.0 .0.audio.lock().unwrap();
        // Unknown controls are created on first use, so code written for the
        // real card's control names runs unchanged in the simulator.
        if !g.controls.iter().any(|c| c.0.name == name) {
            g.controls.push((
                ControlInfo {
                    name: name.to_string(),
                    kind: ControlKind::Integer,
                    count: values.len().max(1) as u32,
                    min: i32::MIN,
                    max: i32::MAX,
                    items: Vec::new(),
                },
                vec![0; values.len().max(1)],
            ));
        }
        let c = g.controls.iter_mut().find(|c| c.0.name == name).unwrap();
        let (min, max) = (c.0.min, c.0.max);
        for (i, slot) in c.1.iter_mut().enumerate() {
            let v = values.get(i).or(values.first()).copied().unwrap_or(0);
            *slot = v.clamp(min, max);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ EHS

impl Ehs for SimDev {
    fn open(&mut self, _config: SerialConfig) -> Result<Box<dyn EhsPort>> {
        if self.0 .0.ehs_open.swap(true, Ordering::SeqCst) {
            return Err(HalError::Invalid("EHS port already open".into()));
        }
        Ok(Box::new(SimEhsPort(self.0.clone())))
    }
}

struct SimEhsPort(SimHandle);

impl EhsPort for SimEhsPort {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let sh = &self.0 .0;
        let deadline = Instant::now() + timeout;
        let mut g = sh.ehs_rx.lock().unwrap();
        while g.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return Ok(0);
            }
            g = sh.ehs_rx_cv.wait_timeout(g, deadline - now).unwrap().0;
        }
        let n = buf.len().min(g.len());
        for b in buf[..n].iter_mut() {
            *b = g.pop_front().unwrap();
        }
        Ok(n)
    }
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.0 .0.ehs_tx.lock().unwrap().extend_from_slice(data);
        Ok(())
    }
    fn configure(&mut self, _: SerialConfig) -> Result<()> {
        Ok(())
    }
    fn set_lines(&mut self, _: bool, _: bool) -> Result<()> {
        Ok(())
    }
}

impl Drop for SimEhsPort {
    fn drop(&mut self) {
        self.0 .0.ehs_open.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_flush_and_capture() {
        for model in Model::ALL {
            let mut hal = Hal::sim(model);
            let sim = hal.sim.clone().unwrap();
            let info = hal.display.info(Screen::Main).unwrap();
            assert_eq!(info.size(), model.main_screen());
            assert_eq!(hal.display.info(Screen::Side).is_some(), model == Model::D70);
            let fb = hal.display.frame(Screen::Main).unwrap();
            assert_eq!(fb.len(), info.len());
            fb.fill(0xaa);
            hal.display.flush(Screen::Main, &[Rect::new(1, 1, 2, 1)]).unwrap();
            let f = sim.frame(Screen::Main).unwrap();
            let bpp = info.format.bytes_per_pixel();
            assert_eq!(f.pixels.iter().filter(|p| **p == 0xaa).count(), 2 * bpp);
            hal.display.flush(Screen::Main, &[]).unwrap();
            assert!(hal.display.capture(Screen::Main).unwrap().pixels.iter().all(|p| *p == 0xaa));
            assert_eq!(sim.frame_generation(), 2);
        }
        let mut hal = Hal::sim(Model::D70);
        assert_eq!(hal.display.frame(Screen::Side).unwrap().len(), 113 * 357);
    }

    #[test]
    fn a_panel_starts_off_and_comes_on_with_the_frame() {
        let mut hal = Hal::sim(Model::D40);
        let sim = hal.sim.clone().unwrap();
        assert_eq!(sim.screen_power(Screen::Main), Some(false));
        hal.display.frame(Screen::Main).unwrap().fill(0x55);
        hal.backlight.set_brightness(Screen::Main, 8).unwrap();
        hal.display.set_power(Screen::Main, true).unwrap();
        // No flush was needed: turning the panel on shows the buffer.
        assert!(sim.first_lit(Screen::Main).unwrap().pixels.iter().all(|p| *p == 0x55));
        assert!(sim.frame(Screen::Main).unwrap().pixels.iter().all(|p| *p == 0x55));
        assert_eq!(
            sim.display_events(),
            [SimDisplayEvent::Brightness(Screen::Main, 8), SimDisplayEvent::Power(Screen::Main, true)]
        );
        assert!(Hal::sim(Model::D60).display.frame(Screen::Side).is_err());
    }

    #[test]
    fn input_lamps_backlight_watchdog() {
        let mut hal = Hal::sim(Model::D50);
        let sim = hal.sim.clone().unwrap();
        assert_eq!(hal.input.next_event(Some(Duration::ZERO)).unwrap(), None);
        sim.tap(Key::Digit(5));
        sim.set_hook(true);
        assert_eq!(
            hal.input.next_event(None).unwrap(),
            Some(InputEvent::Key { key: Key::Digit(5), down: true })
        );
        assert_eq!(
            hal.input.next_event(None).unwrap(),
            Some(InputEvent::Key { key: Key::Digit(5), down: false })
        );
        assert_eq!(hal.input.next_event(None).unwrap(), Some(InputEvent::Hook { off_hook: true }));
        assert!(hal.input.off_hook().unwrap());

        assert_eq!(hal.lamps.lamps().len(), 4 + 10 + 4);
        let mut blinker = Blinker::new();
        blinker.set(Lamp::Line(0), LampState::blinking(LampColor::Green, Blink::Slow));
        blinker.set(Lamp::Message, LampState::solid(LampColor::Red));
        assert!(blinker.needs_ticks());
        blinker.tick(0, hal.lamps.as_mut()).unwrap();
        assert_eq!(sim.lamp(Lamp::Line(0)), LampColor::Green);
        assert_eq!(sim.lamp(Lamp::Message), LampColor::Red);
        blinker.tick(600, hal.lamps.as_mut()).unwrap();
        assert_eq!(sim.lamp(Lamp::Line(0)), LampColor::Off);
        assert_eq!(sim.lamp(Lamp::Message), LampColor::Red);
        blinker.set(Lamp::Line(0), LampState::OFF);
        assert!(!blinker.needs_ticks());

        hal.backlight.set_brightness(Screen::Main, 200).unwrap();
        assert_eq!(sim.brightness(Screen::Main), Some(31));
        hal.backlight.set_contrast(Screen::Main, 7).unwrap();
        assert_eq!(hal.backlight.contrast(Screen::Main).unwrap(), 7);
        assert!(Hal::sim(Model::D60).backlight.set_contrast(Screen::Main, 1).is_err());

        assert!(hal.watchdog.feed().is_err());
        assert_eq!(hal.watchdog.start(30).unwrap(), 30);
        hal.watchdog.feed().unwrap();
        assert_eq!(sim.watchdog().feeds, 1);

        let mut a = [0u8; 13];
        hal.rng.fill(&mut a).unwrap();
        assert!(a.iter().any(|b| *b != 0));
    }

    #[test]
    fn mtd_skip_bad_round_trip() {
        let mut hal = Hal::sim(Model::D60);
        let sim = hal.sim.clone().unwrap();
        let names: Vec<String> = hal.mtd.partitions().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["boot1", "boot2", "ctrl", "nvdata", "k0", "k1", "root0", "root1", "userdata"]);
        let mut k0 = hal.mtd.open("k0").unwrap();
        assert_eq!(k0.info().size, 0x20_0000);
        sim.mtd_set_bad("k0", 1, true);
        assert_eq!(erase_all(k0.as_mut(), |_, _| {}).unwrap(), 1);
        let bs = SIM_ERASE_SIZE as usize;
        let data: Vec<u8> = (0..bs * 2 + 100).map(|i| (i * 7) as u8).collect();
        let end = write_skip_bad(k0.as_mut(), 0, &data, |_, _| {}).unwrap();
        assert_eq!(end, 4 * bs as u64); // blocks 0, 2, 3 used; 1 skipped
        let mut back = vec![0u8; data.len()];
        assert_eq!(read_skip_bad(k0.as_mut(), 0, &mut back).unwrap(), end);
        assert_eq!(back, data);
        // Block 1 untouched, and the raw layout shows the skip.
        let raw = sim.mtd_contents("k0").unwrap();
        assert!(raw[bs..2 * bs].iter().all(|b| *b == 0xff));
        assert_eq!(&raw[2 * bs..3 * bs], &data[bs..2 * bs]);
        // Writing without erasing can only clear bits.
        k0.write(0, &[0xff; 4]).unwrap();
        let mut four = [0u8; 4];
        k0.read(0, &mut four).unwrap();
        assert_eq!(&four, &data[..4]);
        assert!(matches!(k0.erase(bs as u64), Err(HalError::BadBlock(_))));
        assert!(k0.write(1, &[0]).is_err());
        assert!(k0.erase(5).is_err());
        assert!(hal.mtd.open("nope").is_err());
        assert!(write_skip_bad(k0.as_mut(), 0, &vec![0; 0x20_0000], |_, _| {}).is_err());
        assert_eq!(sim.mtd_erase_count("k0", 0), 1);
    }

    #[test]
    fn audio_loopback_and_exclusion() {
        let mut hal = Hal::sim(Model::D60);
        let sim = hal.sim.clone().unwrap();
        sim.audio_set_realtime(false);
        assert_eq!(hal.audio.rate(), 32_000);
        let cfg = PcmConfig::voice(Model::D60);
        assert_eq!(cfg.period_frames, 160);
        let mut spk = hal.audio.open(PcmDevice::Speaker, cfg).unwrap();
        assert!(hal.audio.open(PcmDevice::Headset, cfg).is_err());
        assert!(hal.audio.open(PcmDevice::Speaker, cfg).is_err());
        let mut hs = hal.audio.open(PcmDevice::Handset, cfg).unwrap();
        let mut buf = [1i16; 160];
        spk.read(&mut buf).unwrap();
        assert!(buf.iter().all(|s| *s == 0));
        sim.audio_feed(PcmDevice::Handset, &[7; 160]);
        hs.read(&mut buf).unwrap();
        assert!(buf.iter().all(|s| *s == 7));
        spk.write(&[3; 160]).unwrap();
        assert_eq!(sim.audio_played(PcmDevice::Speaker), vec![3; 160]);
        sim.audio_set_loopback(true);
        spk.write(&[9; 160]).unwrap();
        spk.read(&mut buf).unwrap();
        assert!(buf.iter().all(|s| *s == 9));
        drop(spk);
        assert!(!sim.audio_is_open(PcmDevice::Speaker));
        hal.audio.open(PcmDevice::Headset, cfg).unwrap();

        let mut mixer = hal.audio.mixer().unwrap();
        mixer.set("Sim Playback Volume", &[500]).unwrap();
        assert_eq!(mixer.get("Sim Playback Volume").unwrap(), vec![100]);
        mixer.set("Handset Playback Volume", &[4]).unwrap();
        assert_eq!(mixer.get("Handset Playback Volume").unwrap(), vec![4]);
        assert!(mixer.get("nope").is_err());
    }

    #[test]
    fn audio_realtime_paces() {
        let mut hal = Hal::sim(Model::D40);
        let mut pcm = hal.audio.open(PcmDevice::Handset, PcmConfig::voice(Model::D40)).unwrap();
        let mut buf = [0i16; 80];
        let t = Instant::now();
        for _ in 0..10 {
            pcm.read(&mut buf).unwrap();
        }
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(45), "{e:?}");
    }

    #[test]
    fn board_gains_through_sim_mixer() {
        let mut hal = Hal::sim(Model::D60);
        let mut mixer = hal.audio.mixer().unwrap();
        apply_board_gains(mixer.as_mut(), Model::D60, PcmDevice::Handset, 0).unwrap();
        assert_eq!(mixer.get("CH0 Mic Capture Volume").unwrap(), vec![21]);
        assert_eq!(mixer.get("CH0 Out A Playback Volume").unwrap(), vec![4]); // -9 dB
        assert_eq!(mixer.get("CH0 Sidetone Volume").unwrap(), vec![28]); // -22 dB
        assert_eq!(mixer.get("CH0 Sidetone Switch").unwrap(), vec![1]);
        apply_board_gains(mixer.as_mut(), Model::D60, PcmDevice::Headset, -3).unwrap();
        assert_eq!(mixer.get("CH1 Out B Playback Volume").unwrap(), vec![3]); // -12 dB
        apply_board_gains(mixer.as_mut(), Model::D70, PcmDevice::Speaker, 0).unwrap();
        assert_eq!(mixer.get("CH1 Mic Capture Volume").unwrap(), vec![35]);
        assert_eq!(mixer.get("CH1 Sidetone Switch").unwrap(), vec![0]);
        assert_eq!(board_gains(Model::D40, PcmDevice::Headset).playback_digital_db, -3);
        assert_eq!(out_volume_value(0), 7);
        assert_eq!(out_volume_value(-21), 0);
    }

    #[test]
    fn ehs_port() {
        let mut hal = Hal::sim(Model::D60);
        let sim = hal.sim.clone().unwrap();
        let mut port = hal.ehs.open(SerialConfig::default()).unwrap();
        assert!(hal.ehs.open(SerialConfig::default()).is_err());
        let mut buf = [0u8; 8];
        assert_eq!(port.read(&mut buf, Duration::from_millis(1)).unwrap(), 0);
        sim.ehs_feed(b"abc");
        assert_eq!(port.read(&mut buf, Duration::from_millis(1)).unwrap(), 3);
        port.write(b"xy").unwrap();
        assert_eq!(sim.ehs_written(), b"xy");
    }
}
