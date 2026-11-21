//! Model identity and the per-model facts every subsystem needs.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The supported phones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Model {
    D40,
    D45,
    D50,
    D60,
    D62,
    D65,
    D70,
    D80,
}

/// Hardware family.  Old: grayscale, 16 kHz audio.  New: color, 32 kHz audio.
/// Touch: the D80, which has a touchscreen and no keys, and is not built on
/// the processors of the other two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Family {
    Old,
    New,
    Touch,
}

/// UI skin class (plan section 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Skin {
    /// D40, D45, D50: 240x120, four gray levels.
    SmallGray,
    /// D70: 320x160 plus the 113x357 side display.
    LargeGray,
    /// D60, D62, D65: 480x272 color.
    Color,
    /// D80: 800x1280 color, upright, with a touchscreen: everything on it
    /// is operated by touching it.
    Touch,
}

/// A size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

impl Size {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// System-on-chip variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Soc {
    Bcm11170,
    Bcm11109,
    Bcm11189,
    Bcm11188,
    /// The D80's processor, which is not of the BCMRING family.  Which it
    /// is, is to be settled when the hardware is brought up.
    D80,
}

/// Error from parsing a model name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownModel(pub String);

impl fmt::Display for UnknownModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown phone model {:?}", self.0)
    }
}
impl std::error::Error for UnknownModel {}

impl Model {
    /// Every model, in numeric order.
    pub const ALL: [Model; 8] = [
        Model::D40,
        Model::D45,
        Model::D50,
        Model::D60,
        Model::D62,
        Model::D65,
        Model::D70,
        Model::D80,
    ];

    /// The name as it appears in `digium_board=`, the EFF label, and the
    /// `User-Agent` (`D60`).
    pub const fn name(self) -> &'static str {
        match self {
            Model::D40 => "D40",
            Model::D45 => "D45",
            Model::D50 => "D50",
            Model::D60 => "D60",
            Model::D62 => "D62",
            Model::D65 => "D65",
            Model::D70 => "D70",
            Model::D80 => "D80",
        }
    }

    /// Finds `digium_board=<model>` on a kernel command line.
    pub fn from_cmdline(cmdline: &str) -> Option<Model> {
        cmdline
            .split_ascii_whitespace()
            .find_map(|w| w.strip_prefix("digium_board="))
            .and_then(|v| v.parse().ok())
    }

    /// Matches a devicetree `compatible` string such as `digium,d60`.
    pub fn from_compatible(compatible: &str) -> Option<Model> {
        let c = compatible.trim_end_matches('\0');
        let tail = c.strip_prefix("digium,")?;
        tail.parse().ok()
    }

    pub const fn family(self) -> Family {
        match self {
            Model::D40 | Model::D45 | Model::D50 | Model::D70 => Family::Old,
            Model::D60 | Model::D62 | Model::D65 => Family::New,
            Model::D80 => Family::Touch,
        }
    }

    pub const fn skin(self) -> Skin {
        match self {
            Model::D40 | Model::D45 | Model::D50 => Skin::SmallGray,
            Model::D70 => Skin::LargeGray,
            Model::D60 | Model::D62 | Model::D65 => Skin::Color,
            Model::D80 => Skin::Touch,
        }
    }

    pub const fn soc(self) -> Soc {
        match self {
            Model::D40 | Model::D50 => Soc::Bcm11170,
            Model::D45 | Model::D70 => Soc::Bcm11109,
            Model::D60 => Soc::Bcm11189,
            Model::D62 | Model::D65 => Soc::Bcm11188,
            Model::D80 => Soc::D80,
        }
    }

    /// True for a color panel (XRGB8888); false for gray (R8, four levels).
    pub const fn is_color(self) -> bool {
        matches!(self.skin(), Skin::Color | Skin::Touch)
    }

    /// True for the model whose screen is a touchscreen, and which has no
    /// keys: the D80.
    pub const fn is_touch(self) -> bool {
        matches!(self.skin(), Skin::Touch)
    }

    /// Main display size.
    pub const fn main_screen(self) -> Size {
        match self.skin() {
            Skin::SmallGray => Size::new(240, 120),
            Skin::LargeGray => Size::new(320, 160),
            Skin::Color => Size::new(480, 272),
            Skin::Touch => Size::new(800, 1280),
        }
    }

    /// The D70's rapid-dial side display.
    pub const fn side_screen(self) -> Option<Size> {
        match self {
            Model::D70 => Some(Size::new(113, 357)),
            _ => None,
        }
    }

    /// Height of the softkey bar at the bottom of the main screen.  On the
    /// touchscreen: of one row of softkey buttons.
    pub const fn softkey_bar_height(self) -> u16 {
        match self.skin() {
            Skin::SmallGray => 18,
            Skin::LargeGray => 20,
            Skin::Color => 29,
            Skin::Touch => 120,
        }
    }

    /// The area apps lay out in (main screen minus the softkey bar).
    pub const fn app_content_area(self) -> Size {
        let s = self.main_screen();
        Size::new(s.width, s.height - self.softkey_bar_height())
    }

    /// Number of line keys beside the screen.  The D80 has none.
    pub const fn line_keys(self) -> u8 {
        match self {
            Model::D40 | Model::D45 | Model::D60 | Model::D62 => 2,
            Model::D50 => 4,
            Model::D65 | Model::D70 => 6,
            Model::D80 => 0,
        }
    }

    /// Number of lines the screen shows: one beside each line key, and on
    /// the D80, which has no line keys, the lines drawn on its screen
    /// (touching one does what the line key of a model that has it does:
    /// [`crate::Key::Line`]).  The call core's key table
    /// ([`crate::Keys::line_keys`]) has this many positions.
    ///
    /// The D80's number is a choice, not a fact of the hardware, and is to
    /// be settled with the hardware.
    pub const fn lines(self) -> u8 {
        match self {
            Model::D80 => 6,
            _ => self.line_keys(),
        }
    }

    /// Number of SIP accounts the factory layer allows.
    pub const fn max_accounts(self) -> u8 {
        self.lines()
    }

    /// Dedicated BLF/rapid-dial keys (D50 and D70 have ten).
    pub const fn blf_keys(self) -> u8 {
        match self {
            Model::D50 | Model::D70 => 10,
            _ => 0,
        }
    }

    /// Pages of rapid-dial positions (D70: ten pages on the side LCD; D65
    /// pages its line keys).
    pub const fn blf_pages(self) -> u8 {
        match self {
            Model::D70 => 10,
            Model::D65 => 20,
            Model::D50 => 1,
            _ => 0,
        }
    }

    /// Line lamps present in the model's lamp table (each is red + green).
    pub const fn line_lamps(self) -> u8 {
        match self {
            Model::D40 | Model::D45 => 2,
            Model::D50 => 4,
            Model::D70 => 6,
            // The D6x table lists six; a D60 has fewer fitted.
            Model::D60 | Model::D62 | Model::D65 => 6,
            // Not established: the D80 is taken to have no lamps.
            Model::D80 => 0,
        }
    }

    /// BLF key lamps.
    pub const fn blf_lamps(self) -> u8 {
        self.blf_keys()
    }

    pub const fn has_conference_key(self) -> bool {
        matches!(self, Model::D50 | Model::D70)
    }

    pub const fn gigabit(self) -> bool {
        matches!(self, Model::D45 | Model::D70 | Model::D62 | Model::D65 | Model::D80)
    }

    pub const fn bluetooth(self) -> bool {
        // The D80: by its data sheet; not yet seen on the hardware.
        matches!(self, Model::D65 | Model::D80)
    }

    /// EXP100/EXP150 expansion modules (new family).
    pub const fn expansion_modules(self) -> bool {
        matches!(self.family(), Family::New)
    }

    /// Native sample rate of the audio front end, in Hz.
    pub const fn audio_rate(self) -> u32 {
        match self.family() {
            Family::Old => 16_000,
            Family::New => 32_000,
            // Not the hardware's: the new family's, whose voice processor
            // the simulator stands in for.  To be settled with the D80's
            // audio.
            Family::Touch => 32_000,
        }
    }

    /// Highest backlight level.
    pub const fn backlight_max(self) -> u8 {
        match self.family() {
            Family::Old => 31,
            Family::New => 32,
            // A guess (the range of an Android backlight), to be settled
            // with the hardware.
            Family::Touch => 255,
        }
    }

    /// Installed RAM in MiB.
    pub const fn ram_mib(self) -> u16 {
        match self {
            Model::D40 | Model::D45 | Model::D50 | Model::D60 => 128,
            Model::D62 | Model::D65 | Model::D70 => 256,
            // A guess, to be settled with the hardware.
            Model::D80 => 1024,
        }
    }

    /// `Digium <model> <version>`: the shape DPMA and Asterisk's
    /// `digium_presence` supplement depend on.
    pub fn user_agent(self, version: &crate::FirmwareVersion) -> String {
        format!("Digium {} {}", self.name(), version.underscored())
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Model {
    type Err = UnknownModel;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        Model::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(t))
            .ok_or_else(|| UnknownModel(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline() {
        let c = "console=ttyAMA0,115200n8 vmalloc=256m  digium_board=D60 digium_boot_rev=3 root=/dev/mtdblock7";
        assert_eq!(Model::from_cmdline(c), Some(Model::D60));
        assert_eq!(Model::from_cmdline("root=/dev/sda"), None);
        assert_eq!(Model::from_compatible("digium,d70\0"), Some(Model::D70));
    }

    #[test]
    fn facts() {
        assert_eq!(Model::D40.app_content_area(), Size::new(240, 102));
        assert_eq!(Model::D70.app_content_area(), Size::new(320, 140));
        assert_eq!(Model::D60.app_content_area(), Size::new(480, 243));
        assert_eq!(Model::D50.max_accounts(), 4);
        assert_eq!(Model::D65.audio_rate(), 32_000);
        assert!(Model::D45.gigabit() && !Model::D40.gigabit());
        for m in Model::ALL {
            assert_eq!(m.name().parse::<Model>().unwrap(), m);
        }
    }
}
