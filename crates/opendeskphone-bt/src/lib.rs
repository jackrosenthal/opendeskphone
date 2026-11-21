//! Bluetooth headsets on the D65.
//!
//! The phone is the audio gateway of the Hands-Free and Headset profiles,
//! as the stock firmware's was: a paired headset rings, answers, hangs up,
//! redials, and carries the call audio.
//!
//! The kernel supplies the controller (`hci_uart` with the Broadcom
//! protocol on UART B), L2CAP, RFCOMM, and SCO.  Everything above that is
//! here, in the phone's one process, with no BlueZ daemon:
//!
//! - [`mgmt`]: the kernel's Bluetooth management interface (power,
//!   discovery, pairing, link keys);
//! - [`sdp`]: the two service records a headset looks for, and the query
//!   for a headset's RFCOMM channel;
//! - [`hfp`]: the gateway side of the AT command dialogue;
//! - [`store`]: paired devices under `/nvdata/opendeskphone/bluetooth`;
//! - [`service`]: the thread that owns the sockets and ties these together.
//!
//! Voice does not pass through this crate.  The controller's PCM pins go to
//! the SoC's PCM/TDM interface, which the kernel exposes as the ALSA card
//! named [`ALSA_CARD`]: one mono S16_LE device at 8 kHz.  While
//! [`Event::AudioOpened`] holds, that device carries the headset's
//! microphone and earpiece.

pub mod hfp;
pub mod mgmt;
pub mod sdp;
pub mod service;
pub mod sock;
pub mod store;

use std::fmt;
use std::str::FromStr;

pub use hfp::CallState;
pub use service::{Bluetooth, Command, Config, Event, Profile, SCO_PCM_PARAMS_D65, SCO_PCM_PARAMS_D80};

/// Name of the ALSA card that carries headset audio.
pub const ALSA_CARD: &str = "Digium Bluetooth";

/// Kernel modules Bluetooth needs on a D65, in load order, after the common
/// audio modules (`kernel/README.md`).
pub const KERNEL_MODULES: &[&str] = &[
    "crypto",
    "crypto_algapi",
    "kpp",
    "ecc",
    "ecdh_generic",
    "crc16",
    "libcryptoutils",
    "libaes",
    "bluetooth",
    "rfcomm",
    "btbcm",
    "hci_uart",
    "snd-soc-bt-sco",
    "snd-soc-simple-card-utils",
    "snd-soc-simple-card",
    "snd-soc-bcmring-pcm",
];

/// A Bluetooth device address, stored as the kernel passes it (least
/// significant byte first).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Addr(pub [u8; 6]);

impl Addr {
    pub const ANY: Addr = Addr([0; 6]);
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        write!(f, "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", b[5], b[4], b[3], b[2], b[1], b[0])
    }
}

impl fmt::Debug for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Addr {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        let mut out = [0u8; 6];
        let mut parts = s.split(':');
        for i in (0..6).rev() {
            let part = parts.next().ok_or(())?;
            if part.len() != 2 {
                return Err(());
            }
            out[i] = u8::from_str_radix(part, 16).map_err(|_| ())?;
        }
        if parts.next().is_some() {
            return Err(());
        }
        Ok(Addr(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_text_round_trip() {
        let a: Addr = "00:1B:DC:0F:A1:02".parse().unwrap();
        assert_eq!(a.0, [0x02, 0xa1, 0x0f, 0xdc, 0x1b, 0x00]);
        assert_eq!(a.to_string(), "00:1B:DC:0F:A1:02");
        assert!("00:1B:DC:0F:A1".parse::<Addr>().is_err());
        assert!("00:1B:DC:0F:A1:02:03".parse::<Addr>().is_err());
        assert!("0:1B:DC:0F:A1:02".parse::<Addr>().is_err());
    }
}
