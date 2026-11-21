//! Audio PCM devices and mixer controls.
//!
//! The card ("Digium Phone") has three PCM devices, each mono, 16-bit, at
//! the family's native rate (16 kHz old family, 32 kHz new family):
//! speaker (0), handset (1), headset (2).  Speaker and headset share an APM
//! channel and cannot be open together.

use crate::{AudioPath, Model, Result};

/// One of the three PCM devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PcmDevice {
    Speaker,
    Handset,
    Headset,
}

impl PcmDevice {
    /// ALSA device number on the card.
    pub const fn index(self) -> u32 {
        match self {
            PcmDevice::Speaker => 0,
            PcmDevice::Handset => 1,
            PcmDevice::Headset => 2,
        }
    }
    /// The device behind an audio path.
    pub const fn for_path(path: AudioPath) -> Option<PcmDevice> {
        match path {
            AudioPath::None => None,
            AudioPath::Handset => Some(PcmDevice::Handset),
            AudioPath::Headset => Some(PcmDevice::Headset),
            AudioPath::Speaker => Some(PcmDevice::Speaker),
        }
    }
    /// Speaker and headset exclude each other.
    pub const fn conflicts_with(self, other: PcmDevice) -> bool {
        matches!(
            (self, other),
            (PcmDevice::Speaker, PcmDevice::Headset) | (PcmDevice::Headset, PcmDevice::Speaker)
        )
    }
}

/// Stream parameters.  Samples are always mono `i16` at [`Audio::rate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmConfig {
    /// Frames per period (per `read`/`write` in the audio loop).
    pub period_frames: u32,
    /// Periods in the ring buffer.
    pub periods: u32,
    /// Open the capture direction.
    pub capture: bool,
    /// Open the playback direction.
    pub playback: bool,
}

impl PcmConfig {
    /// Duplex with 5 ms periods (the DSP frame) for `model`, and 40 ms of
    /// buffer: playback starts with 20 ms queued (two ticks of the audio
    /// loop), and capture holds up to four ticks while the loop is late.
    pub fn voice(model: Model) -> PcmConfig {
        PcmConfig {
            period_frames: model.audio_rate() / 200,
            periods: 8,
            capture: true,
            playback: true,
        }
    }
}

/// An open PCM device (one or both directions).
pub trait Pcm: Send {
    /// The configuration in force (the driver may adjust the request).
    fn config(&self) -> PcmConfig;
    /// Reads captured samples, blocking until `buf` is full.  With
    /// `period_frames` samples this paces the audio loop.
    /// [`crate::HalError::Xrun`] on overrun: call [`Pcm::recover`].
    fn read(&mut self, buf: &mut [i16]) -> Result<()>;
    /// Queues samples for playback, blocking while the buffer is full.
    /// [`crate::HalError::Xrun`] on underrun: call [`Pcm::recover`].
    fn write(&mut self, buf: &[i16]) -> Result<()>;
    /// Restarts the streams after an xrun.
    fn recover(&mut self) -> Result<()>;
    /// Playback frames queued and not yet played.
    fn playback_delay(&mut self) -> Result<u32>;
}

/// A mixer control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlInfo {
    pub name: String,
    pub kind: ControlKind,
    /// Number of values (channels).
    pub count: u32,
    pub min: i32,
    pub max: i32,
    /// Item names of an enumerated control.
    pub items: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Boolean,
    Integer,
    Enumerated,
    Other,
}

/// Mixer controls, addressed by name (gains, sidetone, mic mute).
pub trait Mixer: Send {
    fn controls(&mut self) -> Result<Vec<ControlInfo>>;
    fn get(&mut self, name: &str) -> Result<Vec<i32>>;
    /// Sets all values of a control; a single value is applied to every
    /// channel.
    fn set(&mut self, name: &str, values: &[i32]) -> Result<()>;
}

/// Name of the ALSA card that carries Bluetooth headset audio (D65).
pub const BLUETOOTH_CARD: &str = "Digium Bluetooth";
/// Sample rate of the Bluetooth voice device.
pub const BLUETOOTH_RATE: u32 = 8000;

/// The sound card.
pub trait Audio: Send {
    /// Native sample rate in Hz.
    fn rate(&self) -> u32;
    /// Opens a device.  Fails with [`crate::HalError::Invalid`] if a
    /// conflicting device is open.
    fn open(&mut self, device: PcmDevice, config: PcmConfig) -> Result<Box<dyn Pcm>>;
    /// Opens the Bluetooth headset's voice device: the card named
    /// [`BLUETOOTH_CARD`] (D65), mono, 16-bit, at [`BLUETOOTH_RATE`].  It
    /// carries the headset's microphone and earpiece while the Bluetooth
    /// voice link is up.  Other models and backends have none.
    ///
    /// On the D80 there is no such card: the voice processor exchanges
    /// the audio with the Bluetooth module itself.  This opens the phone's
    /// own device, at [`Audio::rate`], with the voice processor on its
    /// Bluetooth path, exchanging audio with the module at 8 kHz, or with
    /// `wideband` (a wide-band speech link) at 16 kHz.
    fn open_bluetooth(&mut self, _config: PcmConfig, _wideband: bool) -> Result<Box<dyn Pcm>> {
        Err(crate::HalError::Unsupported("Bluetooth audio"))
    }
    fn mixer(&mut self) -> Result<Box<dyn Mixer>>;
}

/// Per-model analog and digital gains for one device, from the stock
/// kernels' board table (`BCMRING_CUSTOM_halaudio_dev_list`).  The mainline
/// driver does not apply these; userspace does, with
/// [`apply_board_gains`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardGains {
    /// APM channel: 0 or 1.
    pub channel: u8,
    /// Output driver port: `false` A, `true` B.  (The microphone input
    /// follows the same port and is selected by the card's routing.)
    pub port_b: bool,
    /// Microphone PGA gain in dB (0..=42).
    pub mic_db: i8,
    /// Output driver gain in dB (-21..=0, steps of 3).
    pub out_db: i8,
    /// DAC digital gain in dB.
    pub playback_digital_db: i8,
    /// Sidetone in dB, or `None` for off.
    pub sidetone_db: Option<i8>,
}

/// The board-table gains for a model and device.
pub fn board_gains(model: Model, device: PcmDevice) -> BoardGains {
    let new = matches!(model.family(), opendeskphone_state::Family::New);
    let g = |channel, port_b, mic_db, out_db, playback_digital_db, sidetone_db| BoardGains {
        channel,
        port_b,
        mic_db,
        out_db,
        playback_digital_db,
        sidetone_db,
    };
    match (device, new) {
        (PcmDevice::Handset, false) => g(0, false, 26, -3, 0, Some(-25)),
        (PcmDevice::Handset, true) => g(0, false, 21, -9, 0, Some(-22)),
        (PcmDevice::Headset, false) => g(1, false, 30, -15, -3, Some(-15)),
        (PcmDevice::Headset, true) => g(1, true, 30, -9, 0, Some(-15)),
        (PcmDevice::Speaker, true) => g(1, false, 28, -12, 0, None),
        (PcmDevice::Speaker, false) => match model {
            Model::D45 => g(1, true, 33, -12, 0, None),
            Model::D50 => g(1, false, 33, -15, 0, None),
            Model::D70 => g(1, true, 35, -15, 0, None),
            _ => g(1, false, 33, -12, 0, None),
        },
    }
}

/// Offset of the driver's digital gain controls: value = dB + 50
/// (range -50..=+18 dB).
pub const DIGITAL_GAIN_OFFSET: i32 = 50;

/// Control value of an "Out" volume for a gain in dB (-21..=0 dB in 3 dB
/// steps map to 0..=7).
pub fn out_volume_value(db: i8) -> i32 {
    ((db as i32 + 21) / 3).clamp(0, 7)
}

/// Writes a device's board-table gains to the card's mixer controls
/// (`bcmring-apm` names).  Call when a path is opened.  `out_adjust_db`
/// shifts the output driver gain for the user's volume setting, in dB.
pub fn apply_board_gains(
    mixer: &mut dyn Mixer,
    model: Model,
    device: PcmDevice,
    out_adjust_db: i8,
) -> Result<()> {
    let g = board_gains(model, device);
    let ch = g.channel;
    let port = if g.port_b { 'B' } else { 'A' };
    mixer.set(&format!("CH{ch} Mic Capture Volume"), &[g.mic_db as i32])?;
    mixer.set(&format!("CH{ch} Digital Capture Volume"), &[DIGITAL_GAIN_OFFSET])?;
    mixer.set(
        &format!("CH{ch} Digital Playback Volume"),
        &[DIGITAL_GAIN_OFFSET + g.playback_digital_db as i32],
    )?;
    mixer.set(
        &format!("CH{ch} Out {port} Playback Volume"),
        &[out_volume_value(g.out_db.saturating_add(out_adjust_db))],
    )?;
    match g.sidetone_db {
        Some(db) => {
            mixer.set(&format!("CH{ch} Sidetone Volume"), &[DIGITAL_GAIN_OFFSET + db as i32])?;
            mixer.set(&format!("CH{ch} Sidetone Switch"), &[1])?;
        }
        None => mixer.set(&format!("CH{ch} Sidetone Switch"), &[0])?,
    }
    Ok(())
}
