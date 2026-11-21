//! The real hardware, through the mainline kernel port's interfaces.
//!
//! | Function | Interface |
//! | --- | --- |
//! | Display | DRM/KMS dumb buffers, `DIRTYFB` for the SPI panels ([`DrmDisplay`]) |
//! | Backlight, contrast | backlight and LCD class sysfs ([`SysBacklight`]) |
//! | Keys, hook, touchscreen | evdev, found by capability ([`EvdevInput`]) |
//! | Lamps | LED class sysfs ([`SysLamps`]) |
//! | Audio | ALSA PCM and control ioctls ([`AlsaAudio`]) |
//! | Flash | MTD character devices ([`PhoneMtd`]); the D80 has none (its storage is an eMMC) |
//! | Watchdog, RNG | `/dev/watchdog`, `/dev/hwrng` |
//! | EHS | `/dev/ttyAMA0`; on the D80 `/dev/ttymxc1` |

mod alsa;
mod drm;
mod evdev;
mod misc;
mod mtd;
mod sys;
mod sysfs;

pub use alsa::AlsaAudio;
pub use drm::DrmDisplay;
pub use evdev::{key_from_code, EvdevInput};
pub use misc::{PhoneEhs, PhoneRng, PhoneWatchdog};
pub use mtd::PhoneMtd;
pub use sysfs::{SysBacklight, SysLamps};

use crate::null::Null;
use crate::*;

/// Builds the phone HAL; see [`Hal::phone`].  Each device is opened
/// independently; failures are recorded in [`Hal::missing`].
pub fn build(model: Model) -> Hal {
    let mut missing: Vec<(&'static str, String)> = Vec::new();
    macro_rules! dev {
        ($name:literal, $tr:ty, $open:expr) => {{
            let r: Result<_> = $open;
            match r {
                Ok(d) => Box::new(d) as Box<$tr>,
                Err(e) => {
                    log::warn!("HAL: {} unavailable: {e}", $name);
                    missing.push(($name, e.to_string()));
                    Box::new(Null($name)) as Box<$tr>
                }
            }
        }};
    }
    let display = dev!("display", dyn Display, DrmDisplay::open(model));
    let backlight = dev!("backlight", dyn Backlight, SysBacklight::open(model));
    let input = dev!("input", dyn Input, EvdevInput::open(model));
    let lamps = dev!("lamps", dyn Lamps, SysLamps::open());
    let audio = dev!("audio", dyn Audio, AlsaAudio::open(model));
    // The D80's storage is an eMMC: it has no MTD partitions, and their
    // absence is not a fault to report.
    let mtd = if model.is_touch() { Box::new(Null("mtd")) as Box<dyn Mtd> } else { dev!("mtd", dyn Mtd, PhoneMtd::open()) };
    let rng = dev!("rng", dyn Rng, PhoneRng::open());
    Hal {
        model,
        display,
        backlight,
        input,
        lamps,
        audio,
        mtd,
        // Opened on `start`, because opening the device arms it.
        watchdog: Box::new(PhoneWatchdog::new()),
        rng,
        ehs: Box::new(PhoneEhs::for_model(model)),
        sim: None,
        missing,
    }
}

#[cfg(test)]
mod abi_tests {
    //! Struct sizes and ioctl numbers, checked against values printed by a
    //! C program built from the Linux uapi headers for each target
    //! (`arm-linux-musleabi` and `x86_64-linux-musl`).
    use super::alsa::*;
    use super::drm::*;
    use super::evdev::*;
    use super::misc::*;
    use super::mtd::*;

    #[test]
    fn same_on_every_target() {
        assert_eq!(eviocgbit(1, 96), 0x8060_4521);
        assert_eq!(eviocgkey(96), 0x8060_4518);
        assert_eq!(eviocgbit(0, 4), 0x8004_4520);
        assert_eq!(eviocgsw(3), 0x8003_451b);
        assert_eq!(WDIOC_SETTIMEOUT, 0xc004_5706);
        assert_eq!(WDIOC_KEEPALIVE, 0x8004_5705);
        assert_eq!(WDIOC_GETTIMELEFT, 0x8004_570a);
        assert_eq!(MEMGETINFO, 0x8020_4d01);
        assert_eq!(MEMERASE, 0x4008_4d02);
        assert_eq!(MEMGETBADBLOCK, 0x4008_4d0b);
        assert_eq!(MEMSETBADBLOCK, 0x4008_4d0c);
        assert_eq!(SNDRV_PCM_IOCTL_PREPARE, 0x4140);
        assert_eq!(SNDRV_PCM_IOCTL_DROP, 0x4143);
        assert_eq!(SNDRV_CTL_IOCTL_CARD_INFO, 0x8178_5501);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_INFO, 0xc110_5511);
        assert_eq!(DRM_IOCTL_SET_MASTER, 0x641e);
        assert_eq!(DRM_IOCTL_MODE_GETRESOURCES, 0xc040_64a0);
        assert_eq!(DRM_IOCTL_MODE_GETCONNECTOR, 0xc050_64a7);
        assert_eq!(DRM_IOCTL_MODE_GETENCODER, 0xc014_64a6);
        assert_eq!(DRM_IOCTL_MODE_SETCRTC, 0xc068_64a2);
        assert_eq!(DRM_IOCTL_MODE_CREATE_DUMB, 0xc020_64b2);
        assert_eq!(DRM_IOCTL_MODE_MAP_DUMB, 0xc010_64b3);
        assert_eq!(DRM_IOCTL_MODE_DESTROY_DUMB, 0xc004_64b4);
        assert_eq!(DRM_IOCTL_MODE_ADDFB2, 0xc068_64b8);
        assert_eq!(DRM_IOCTL_MODE_RMFB, 0xc004_64af);
        assert_eq!(DRM_IOCTL_MODE_DIRTYFB, 0xc018_64b1);
        assert_eq!(size_of::<DrmModeModeinfo>(), 68);
        assert_eq!(size_of::<DrmClipRect>(), 8);
        assert_eq!(size_of::<SndCtlElemId>(), 64);
        assert_eq!(size_of::<SndCtlElemInfo>(), 272);
        assert_eq!(size_of::<SndCtlCardInfo>(), 376);
        assert_eq!(std::mem::offset_of!(SndPcmHwParams, intervals), 260);
        assert_eq!(std::mem::offset_of!(SndPcmHwParams, rmask), 512);
        assert_eq!(std::mem::offset_of!(SndPcmHwParams, fifo_size), 536);
        assert_eq!(std::mem::offset_of!(SndCtlElemInfo, value), 80);
        assert_eq!(std::mem::offset_of!(SndCtlElemValue, value), 72);
        assert_eq!(DRM_FORMAT_XRGB8888, 0x3432_5258);
        assert_eq!(DRM_FORMAT_R8, 0x2020_3852);
        assert_eq!(KEY_MAX, 0x2ff);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn arm_32_bit() {
        assert_eq!(size_of::<InputEventRaw>(), 16);
        assert_eq!(SNDRV_PCM_IOCTL_HW_PARAMS, 0xc25c_4111);
        assert_eq!(SNDRV_PCM_IOCTL_SW_PARAMS, 0xc068_4113);
        assert_eq!(SNDRV_PCM_IOCTL_DELAY, 0x8004_4121);
        assert_eq!(SNDRV_PCM_IOCTL_WRITEI_FRAMES, 0x400c_4150);
        assert_eq!(SNDRV_PCM_IOCTL_READI_FRAMES, 0x800c_4151);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_LIST, 0xc048_5510);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_READ, 0xc2c8_5512);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_WRITE, 0xc2c8_5513);
        assert_eq!(size_of::<SndPcmHwParams>(), 604);
        assert_eq!(size_of::<SndPcmSwParams>(), 104);
        assert_eq!(size_of::<SndCtlElemValue>(), 712);
        assert_eq!(std::mem::offset_of!(SndPcmSwParams, proto), 40);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn x86_64() {
        assert_eq!(size_of::<InputEventRaw>(), 24);
        assert_eq!(SNDRV_PCM_IOCTL_HW_PARAMS, 0xc260_4111);
        assert_eq!(SNDRV_PCM_IOCTL_SW_PARAMS, 0xc088_4113);
        assert_eq!(SNDRV_PCM_IOCTL_DELAY, 0x8008_4121);
        assert_eq!(SNDRV_PCM_IOCTL_WRITEI_FRAMES, 0x4018_4150);
        assert_eq!(SNDRV_PCM_IOCTL_READI_FRAMES, 0x8018_4151);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_LIST, 0xc050_5510);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_READ, 0xc4c8_5512);
        assert_eq!(SNDRV_CTL_IOCTL_ELEM_WRITE, 0xc4c8_5513);
        assert_eq!(size_of::<SndPcmHwParams>(), 608);
        assert_eq!(size_of::<SndPcmSwParams>(), 136);
        assert_eq!(size_of::<SndCtlElemValue>(), 1224);
        assert_eq!(std::mem::offset_of!(SndPcmSwParams, proto), 72);
    }
}
