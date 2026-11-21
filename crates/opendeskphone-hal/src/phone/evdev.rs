//! Keys, the hookswitch, and the touchscreen through evdev.  Devices are
//! found by capability, never by `event0`.
//!
//! The D40 thru D70 have a keypad whose hookswitch is one of its keys
//! (`KEY_PHONE`).  The D80 has a touchscreen, and its hookswitch is a
//! switch (`EV_SW`) of its GPIO keys device.

use super::sys::*;
use crate::*;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

/// `struct input_event`.  The two time fields are `__kernel_ulong_t` on
/// 32-bit (16 bytes total) and `long` on 64-bit (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct InputEventRaw {
    sec: usize,
    usec: usize,
    type_: u16,
    code: u16,
    value: i32,
}

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const EV_SW: u16 = 5;
const SYN_REPORT: u16 = 0;
/// The first contact's position, which the kernel reports for a
/// touchscreen beside the positions of every contact.
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
/// What tells a touchscreen from other devices with absolute axes.
const ABS_MT_POSITION_X: usize = 0x35;
const ABS_BYTES: usize = 0x3f / 8 + 1;
/// Down while a finger is on the touchscreen.
const BTN_TOUCH: u16 = 0x14a;
/// The D80's hookswitch: the code the stock kernel reports it with, which
/// the devicetree keeps (`SW_LINEIN_INSERT`).
pub(crate) const SW_HOOK: u16 = 13;
const SW_BYTES: usize = 0x10 / 8 + 1;
/// Whether the D80's hookswitch is on (its line low) while the handset is
/// on the cradle.  It is, as seen on the hardware: the switch read 0 with
/// the handset off the cradle, and putting the handset on, taking it off,
/// and putting it on again gave 1, 0, 1.
const SW_HOOK_ON_IS_ON_CRADLE: bool = true;
pub(crate) const KEY_MAX: usize = 0x2ff;
const KEY_BYTES: usize = KEY_MAX / 8 + 1;

pub(crate) const fn eviocgbit(ev: u8, len: usize) -> u32 {
    ioc(IOC_READ, b'E', 0x20 + ev, len)
}
pub(crate) const fn eviocgkey(len: usize) -> u32 {
    ioc(IOC_READ, b'E', 0x18, len)
}
pub(crate) const fn eviocgsw(len: usize) -> u32 {
    ioc(IOC_READ, b'E', 0x1b, len)
}

// Linux key codes used by the port's keymaps (`DIGIUM_KEYS_PROPOSED`).
const KEY_UP: u16 = 103;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;
const KEY_MENU: u16 = 139;
const KEY_VOLUMEDOWN: u16 = 114;
const KEY_VOLUMEUP: u16 = 115;
/// Hookswitch: down while the handset is on the cradle.
pub(crate) const KEY_PHONE: u16 = 169;
const KEY_CANCEL: u16 = 223;
const KEY_MICMUTE: u16 = 248;
const KEY_SELECT: u16 = 0x161;
const KEY_INFO: u16 = 0x166;
const KEY_VOICEMAIL: u16 = 0x1ac;
const KEY_ADDRESSBOOK: u16 = 0x1ad;
const KEY_APPSELECT: u16 = 0x244;
const KEY_NUMERIC_0: u16 = 0x200;
const KEY_NUMERIC_STAR: u16 = 0x20a;
const KEY_NUMERIC_POUND: u16 = 0x20b;
const KEY_MACRO1: u16 = 0x290;
const KEY_KBD_LCD_MENU1: u16 = 0x2b8;

/// Maps a Linux key code from the phone's keypad to a [`Key`].  Returns
/// `None` for the hookswitch code.  A code no keymap uses comes out as
/// [`Key::Unknown`].
///
/// The keys only the D62, D65, and D70 keymaps have (Menu, Status, Apps,
/// Contacts, the two BLF page keys, and line keys 5 and 6) are mapped from
/// the vendor phone application's key table, not from key presses; see
/// `digium-keys.h` in the kernel's devicetree directory.
pub fn key_from_code(code: u16) -> Option<Key> {
    Some(match code {
        KEY_PHONE => return None,
        c @ KEY_NUMERIC_0..=0x209 => Key::Digit((c - KEY_NUMERIC_0) as u8),
        KEY_NUMERIC_STAR => Key::Star,
        KEY_NUMERIC_POUND => Key::Pound,
        KEY_UP => Key::Up,
        KEY_DOWN => Key::Down,
        KEY_LEFT => Key::Left,
        KEY_RIGHT => Key::Right,
        KEY_SELECT => Key::Select,
        KEY_CANCEL => Key::Cancel,
        KEY_VOLUMEDOWN => Key::VolumeDown,
        KEY_VOLUMEUP => Key::VolumeUp,
        c @ KEY_KBD_LCD_MENU1..=0x2bb => Key::Soft((c - KEY_KBD_LCD_MENU1) as u8 + 1),
        KEY_MICMUTE => Key::Mute,
        KEY_VOICEMAIL => Key::Messages,
        KEY_INFO => Key::Info,
        KEY_MENU => Key::Menu,
        KEY_APPSELECT => Key::Apps,
        KEY_ADDRESSBOOK => Key::Contacts,
        c @ KEY_MACRO1..=0x2ad => match c - KEY_MACRO1 + 1 {
            1 => Key::Speaker,
            2 => Key::Headset,
            3 => Key::Transfer,
            4 => Key::Hold,
            5 => Key::Blf(7),
            6 => Key::Redial,
            8 => Key::Blf(9),
            9 => Key::Line(2),
            10 => Key::Line(1),
            11 => Key::Line(4),
            12 => Key::Blf(8),
            13 => Key::Blf(10),
            15 => Key::Conference,
            16 => Key::Line(3),
            n @ 17..=22 => Key::Blf((n - 16) as u8),
            24 => Key::Status,
            26 => Key::BlfPageLeft,
            27 => Key::BlfPageRight,
            28 => Key::Line(5),
            29 => Key::Line(6),
            _ => Key::Unknown(c),
        },
        c => Key::Unknown(c),
    })
}

/// What one touchscreen report has said so far, and what the last whole
/// one said.
#[derive(Default)]
struct Touch {
    x: i32,
    y: i32,
    down: bool,
    /// As last passed on.
    was_down: bool,
    was_at: (u16, u16),
}

struct Dev {
    file: File,
    /// The hookswitch is this device's `KEY_PHONE`.
    has_hook: bool,
    /// The hookswitch is this device's switch [`SW_HOOK`].
    has_hook_switch: bool,
    touch: Option<Touch>,
}

/// The keypad, the hookswitch, and the touchscreen.
pub struct EvdevInput {
    devs: Vec<Dev>,
    pending: VecDeque<InputEvent>,
    /// The main screen's size, for the touchscreen's positions.
    screen: Size,
    /// The panel is mounted upside down, and the touchscreen with it.
    rotated: bool,
}

fn bit(bits: &[u8], n: usize) -> bool {
    bits.get(n / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

/// A touchscreen's position as the pixel of the screen the UI draws.  The
/// touchscreen reports positions of the panel's own picture, which on the
/// D80 is upside down to the viewer (the panel is mounted so, and the
/// display turns the picture: see `DrmDisplay`).
pub(crate) fn touch_point(x: i32, y: i32, screen: Size, rotated: bool) -> (u16, u16) {
    let x = x.clamp(0, i32::from(screen.width) - 1) as u16;
    let y = y.clamp(0, i32::from(screen.height) - 1) as u16;
    if rotated { (screen.width - 1 - x, screen.height - 1 - y) } else { (x, y) }
}

impl Touch {
    /// Takes one event of a report.  At the report's end, returns what the
    /// report as a whole says, if that is news.
    fn event(&mut self, type_: u16, code: u16, value: i32, screen: Size, rotated: bool) -> Option<InputEvent> {
        match (type_, code) {
            (EV_ABS, ABS_X) => self.x = value,
            (EV_ABS, ABS_Y) => self.y = value,
            (EV_KEY, BTN_TOUCH) => self.down = value != 0,
            (EV_SYN, SYN_REPORT) => {
                let at = touch_point(self.x, self.y, screen, rotated);
                let phase = match (self.was_down, self.down) {
                    (false, true) => TouchPhase::Down,
                    (true, true) if at != self.was_at => TouchPhase::Move,
                    (true, false) => TouchPhase::Up,
                    _ => return None,
                };
                // A finger leaves from where it last was: the report that
                // lifts it may carry no position.
                let at = if phase == TouchPhase::Up { self.was_at } else { at };
                self.was_down = self.down;
                self.was_at = at;
                return Some(InputEvent::Touch { phase, x: at.0, y: at.1 });
            }
            _ => {}
        }
        None
    }
}

impl EvdevInput {
    pub fn open(model: Model) -> Result<EvdevInput> {
        let mut devs = Vec::new();
        let mut paths: Vec<_> = std::fs::read_dir("/dev/input")
            .map_err(|e| HalError::io("/dev/input", e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
            .collect();
        paths.sort();
        for p in paths {
            let Ok(file) = open_ro(&p, libc::O_NONBLOCK) else { continue };
            let fd = file.as_raw_fd();
            let mut types = [0u8; 4];
            if ioctl(fd, eviocgbit(0, types.len()), &mut types).is_err() {
                continue;
            }
            let mut keys = [0u8; KEY_BYTES];
            let mut abs = [0u8; ABS_BYTES];
            let mut switches = [0u8; SW_BYTES];
            if bit(&types, EV_KEY as usize) && ioctl(fd, eviocgbit(EV_KEY as u8, keys.len()), &mut keys).is_err() {
                continue;
            }
            if bit(&types, EV_ABS as usize) {
                let _ = ioctl(fd, eviocgbit(EV_ABS as u8, abs.len()), &mut abs);
            }
            if bit(&types, EV_SW as usize) {
                let _ = ioctl(fd, eviocgbit(EV_SW as u8, switches.len()), &mut switches);
            }
            // Only devices that carry what a phone has: the keypad
            // (numeric keys), a hookswitch, or a touchscreen.
            let has_hook = bit(&keys, KEY_PHONE as usize);
            let has_hook_switch = bit(&switches, SW_HOOK as usize);
            let touch = bit(&abs, ABS_MT_POSITION_X).then(Touch::default);
            if !has_hook && !has_hook_switch && touch.is_none() && !bit(&keys, KEY_NUMERIC_0 as usize) {
                continue;
            }
            devs.push(Dev { file, has_hook, has_hook_switch, touch });
        }
        if devs.is_empty() {
            return Err(HalError::Unavailable("no keypad, hookswitch, or touchscreen input device".into()));
        }
        if model.is_touch() && !devs.iter().any(|d| d.touch.is_some()) {
            log::warn!("HAL: no touchscreen among the input devices");
        }
        Ok(EvdevInput { devs, pending: VecDeque::new(), screen: model.main_screen(), rotated: model.is_touch() })
    }

    fn drain(&mut self, idx: usize) -> Result<()> {
        let mut raw = [InputEventRaw::default(); 32];
        loop {
            // SAFETY: `InputEventRaw` is plain data; the byte view covers
            // exactly the array.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(raw.as_mut_ptr() as *mut u8, size_of_val(&raw))
            };
            let n = match self.devs[idx].file.read(bytes) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(HalError::io("read input event", e)),
            };
            for ev in &raw[..n / size_of::<InputEventRaw>()] {
                let dev = &mut self.devs[idx];
                if let Some(touch) = dev.touch.as_mut() {
                    self.pending.extend(touch.event(ev.type_, ev.code, ev.value, self.screen, self.rotated));
                    continue;
                }
                if ev.type_ == EV_SW && ev.code == SW_HOOK && dev.has_hook_switch {
                    self.pending.push_back(InputEvent::Hook { off_hook: (ev.value != 0) != SW_HOOK_ON_IS_ON_CRADLE });
                    continue;
                }
                if ev.type_ != EV_KEY || ev.value > 1 {
                    continue; // not a key, or auto-repeat
                }
                self.pending.push_back(if ev.code == KEY_PHONE {
                    InputEvent::Hook { off_hook: ev.value == 0 }
                } else if let Some(key) = key_from_code(ev.code) {
                    InputEvent::Key { key, down: ev.value == 1 }
                } else {
                    continue;
                });
            }
        }
    }
}

impl Input for EvdevInput {
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<InputEvent>> {
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        let fds: Vec<RawFd> = self.devs.iter().map(|d| d.file.as_raw_fd()).collect();
        let ms = match timeout {
            None => -1,
            Some(t) => t.as_millis().min(i32::MAX as u128) as i32,
        };
        let ready = poll_readable(&fds, ms).map_err(|e| HalError::io("poll input", e))?;
        for (i, r) in ready.into_iter().enumerate() {
            if r {
                self.drain(i)?;
            }
        }
        Ok(self.pending.pop_front())
    }

    fn off_hook(&mut self) -> Result<bool> {
        if let Some(d) = self.devs.iter().find(|d| d.has_hook_switch) {
            let mut switches = [0u8; SW_BYTES];
            ioctl(d.file.as_raw_fd(), eviocgsw(switches.len()), &mut switches)
                .map_err(|e| HalError::io("EVIOCGSW", e))?;
            return Ok(bit(&switches, SW_HOOK as usize) != SW_HOOK_ON_IS_ON_CRADLE);
        }
        let Some(d) = self.devs.iter().find(|d| d.has_hook) else { return Ok(false) };
        let mut keys = [0u8; KEY_BYTES];
        ioctl(d.file.as_raw_fd(), eviocgkey(keys.len()), &mut keys)
            .map_err(|e| HalError::io("EVIOCGKEY", e))?;
        // Key down = handset on the cradle.
        Ok(!bit(&keys, KEY_PHONE as usize))
    }

    fn poll_fds(&self) -> Vec<RawFd> {
        self.devs.iter().map(|d| d.file.as_raw_fd()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_codes() {
        assert_eq!(key_from_code(0x200), Some(Key::Digit(0)));
        assert_eq!(key_from_code(0x209), Some(Key::Digit(9)));
        assert_eq!(key_from_code(0x20a), Some(Key::Star));
        assert_eq!(key_from_code(0x2b8), Some(Key::Soft(1)));
        assert_eq!(key_from_code(0x2bb), Some(Key::Soft(4)));
        assert_eq!(key_from_code(0x290), Some(Key::Speaker));
        assert_eq!(key_from_code(0x290 + 9), Some(Key::Line(1)));
        assert_eq!(key_from_code(0x290 + 8), Some(Key::Line(2)));
        assert_eq!(key_from_code(0x290 + 16), Some(Key::Blf(1)));
        assert_eq!(key_from_code(0x290 + 21), Some(Key::Blf(6)));
        assert_eq!(key_from_code(0x290 + 4), Some(Key::Blf(7)));
        assert_eq!(key_from_code(0x290 + 22), Some(Key::Unknown(0x290 + 22)));
        // The keys of the D62, D65, and D70 keymaps: DGK_MENU, DGK_APPS,
        // DGK_CONTACTS, and KEY_MACRO24, 26, 27, 28, and 29.
        assert_eq!(key_from_code(139), Some(Key::Menu));
        assert_eq!(key_from_code(0x244), Some(Key::Apps));
        assert_eq!(key_from_code(0x1ad), Some(Key::Contacts));
        assert_eq!(key_from_code(0x290 + 23), Some(Key::Status));
        assert_eq!(key_from_code(0x290 + 25), Some(Key::BlfPageLeft));
        assert_eq!(key_from_code(0x290 + 26), Some(Key::BlfPageRight));
        assert_eq!(key_from_code(0x290 + 27), Some(Key::Line(5)));
        assert_eq!(key_from_code(0x290 + 28), Some(Key::Line(6)));
        assert_eq!(key_from_code(0x290 + 24), Some(Key::Unknown(0x290 + 24)));
        assert_eq!(key_from_code(0x290 + 29), Some(Key::Unknown(0x290 + 29)));
        assert_eq!(key_from_code(0x1ac), Some(Key::Messages));
        assert_eq!(key_from_code(169), None);
    }

    /// The D80's panel is upside down to the viewer, and its touchscreen
    /// reports positions of the panel's picture: the viewer's top left
    /// corner is the far corner of both ranges (on the hardware it read
    /// about 712, 1199).
    #[test]
    fn a_touch_is_where_the_viewer_sees_it() {
        let screen = Size::new(800, 1280);
        assert_eq!(touch_point(799, 1279, screen, true), (0, 0));
        assert_eq!(touch_point(0, 0, screen, true), (799, 1279));
        assert_eq!(touch_point(712, 1199, screen, true), (87, 80));
        // The controller's range is one more than the last pixel.
        assert_eq!(touch_point(800, 1280, screen, true), (0, 0));
        assert_eq!(touch_point(-5, 3000, screen, false), (0, 1279));
        assert_eq!(touch_point(10, 20, screen, false), (10, 20));
    }

    #[test]
    fn a_finger_goes_down_moves_and_leaves() {
        let screen = Size::new(800, 1280);
        let mut t = Touch::default();
        let mut feed = |events: &[(u16, u16, i32)]| -> Vec<InputEvent> {
            events.iter().filter_map(|(ty, code, value)| t.event(*ty, *code, *value, screen, false)).collect()
        };
        let report = (EV_SYN, SYN_REPORT, 0);
        assert_eq!(
            feed(&[(EV_ABS, ABS_X, 100), (EV_ABS, ABS_Y, 200), (EV_KEY, BTN_TOUCH, 1), report]),
            [InputEvent::Touch { phase: TouchPhase::Down, x: 100, y: 200 }]
        );
        // A report that changes nothing says nothing.
        assert_eq!(feed(&[report]), []);
        assert_eq!(feed(&[(EV_ABS, ABS_Y, 210), report]), [InputEvent::Touch { phase: TouchPhase::Move, x: 100, y: 210 }]);
        // The finger leaves from where it was.
        assert_eq!(feed(&[(EV_KEY, BTN_TOUCH, 0), report]), [InputEvent::Touch { phase: TouchPhase::Up, x: 100, y: 210 }]);
        assert_eq!(feed(&[report]), []);
    }
}
