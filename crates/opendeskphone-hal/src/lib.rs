//! Hardware abstraction for Open Desk Phone (plan section 6).
//!
//! Traits for each hardware function, with two backends:
//!
//! - [`sim`]: runs on a workstation.  Framebuffers live in memory, keys are
//!   injected, lamps are recorded, flash is simulated.  [`Hal::sim`].
//! - [`phone`]: the real devices of the mainline kernel port (DRM/KMS,
//!   evdev, LED class, ALSA, MTD, watchdog).  [`Hal::phone`].
//!
//! Subsystems take the pieces they own out of the [`Hal`] bundle: the UI
//! thread takes `display`, `backlight`, `input`, and `lamps`; the audio
//! thread takes `audio`; the updater takes `mtd`; the supervisor takes
//! `watchdog`.

mod audio;
mod display;
mod ehs;
mod error;
pub mod expansion;
mod input;
mod lamps;
mod mtd;
mod null;
mod system;

pub mod phone;
pub mod sim;

pub use audio::*;
pub use display::*;
pub use ehs::*;
pub use error::{HalError, Result};
pub use input::*;
pub use lamps::*;
pub use mtd::*;
pub use null::Null;
pub use system::*;

pub use opendeskphone_state::{AudioPath, Blink, Family, Key, LampColor, LampState, Model, Size, TouchPhase};

/// Every hardware function for one phone.  Fields are public so that each
/// subsystem can take ownership of its part (`std::mem::replace` with a
/// null device, or destructure the bundle).
pub struct Hal {
    pub model: Model,
    pub display: Box<dyn Display>,
    pub backlight: Box<dyn Backlight>,
    pub input: Box<dyn Input>,
    pub lamps: Box<dyn Lamps>,
    pub audio: Box<dyn Audio>,
    pub mtd: Box<dyn Mtd>,
    pub watchdog: Box<dyn Watchdog>,
    pub rng: Box<dyn Rng>,
    pub ehs: Box<dyn Ehs>,
    /// Present for the sim backend: the test/mirror side of the simulated
    /// hardware.
    pub sim: Option<sim::SimHandle>,
    /// Phone backend: devices that could not be opened (name, error).  The
    /// corresponding field holds a null device whose calls fail with
    /// [`HalError::Unavailable`].
    pub missing: Vec<(&'static str, String)>,
}

impl Hal {
    /// The simulator backend for `model`.
    pub fn sim(model: Model) -> Hal {
        sim::build(model)
    }

    /// The real hardware.  Never fails as a whole: a device that cannot be
    /// opened is listed in [`Hal::missing`] so that the phone still comes up
    /// (e.g., without audio) and can report what is wrong.
    pub fn phone(model: Model) -> Hal {
        phone::build(model)
    }
}
