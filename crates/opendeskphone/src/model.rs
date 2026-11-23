//! Phone models and how the running one is detected.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// Old family: grayscale, 16 kHz audio.  New family: color, 32 kHz audio.
/// Touch: the D80, color, 16 kHz audio, on an i.MX 6DualLite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Old,
    New,
    Touch,
}

impl Model {
    pub const ALL: [Model; 8] =
        [Model::D40, Model::D45, Model::D50, Model::D60, Model::D62, Model::D65, Model::D70, Model::D80];

    pub fn family(self) -> Family {
        match self {
            Model::D40 | Model::D45 | Model::D50 | Model::D70 => Family::Old,
            Model::D60 | Model::D62 | Model::D65 => Family::New,
            Model::D80 => Family::Touch,
        }
    }

    /// Upper-case name as it appears in `digium_board=` and the User-Agent.
    pub fn name(self) -> &'static str {
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
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Model {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Model::ALL.iter().copied().find(|m| m.name().eq_ignore_ascii_case(s.trim())).ok_or(())
    }
}

impl From<Model> for opendeskphone_config::Model {
    fn from(model: Model) -> Self {
        use opendeskphone_config::Model as C;
        match model {
            Model::D40 => C::D40,
            Model::D45 => C::D45,
            Model::D50 => C::D50,
            Model::D60 => C::D60,
            Model::D62 => C::D62,
            Model::D65 => C::D65,
            Model::D70 => C::D70,
            Model::D80 => C::D80,
        }
    }
}

impl From<Model> for opendeskphone_state::Model {
    fn from(model: Model) -> Self {
        use opendeskphone_state::Model as S;
        match model {
            Model::D40 => S::D40,
            Model::D45 => S::D45,
            Model::D50 => S::D50,
            Model::D60 => S::D60,
            Model::D62 => S::D62,
            Model::D65 => S::D65,
            Model::D70 => S::D70,
            Model::D80 => S::D80,
        }
    }
}

/// `digium_board=` value from a kernel command line.
pub fn model_from_cmdline(cmdline: &str) -> Option<Model> {
    cmdline
        .split_ascii_whitespace()
        .find_map(|w| w.strip_prefix("digium_board="))
        .and_then(|v| v.parse().ok())
}

/// Model from a devicetree `compatible` property (NUL-separated strings such
/// as `digium,d60`).
pub fn model_from_compatible(compatible: &[u8]) -> Option<Model> {
    compatible
        .split(|b| *b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .find_map(|s| s.strip_prefix("digium,"))
        .and_then(|v| v.parse().ok())
}

/// Result of model detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detected {
    pub model: Model,
    /// False when the command line and the devicetree disagree or one of
    /// them gave no answer.
    pub cross_checked: bool,
}

/// Decide the model.  The command line (set by boot2 from the unit's own
/// record) wins; the devicetree (appended to the kernel, so a property of the
/// image) is the cross-check and the fallback.
pub fn decide(cmdline: Option<Model>, devicetree: Option<Model>) -> Option<Detected> {
    match (cmdline, devicetree) {
        (Some(c), Some(d)) => Some(Detected { model: c, cross_checked: c == d }),
        (Some(m), None) | (None, Some(m)) => Some(Detected { model: m, cross_checked: false }),
        (None, None) => None,
    }
}

/// Detect the model of the phone this process runs on.
pub fn detect() -> Option<Detected> {
    let cmdline = std::fs::read_to_string("/proc/cmdline").ok();
    let compat = std::fs::read("/proc/device-tree/compatible").ok();
    decide(
        cmdline.as_deref().and_then(model_from_cmdline),
        compat.as_deref().and_then(model_from_compatible),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline() {
        let c = "console=ttyAMA0,115200n8 vmalloc=256m  digium_board=D60 digium_boot_rev=3 root=/dev/mtdblock7";
        assert_eq!(model_from_cmdline(c), Some(Model::D60));
        assert_eq!(model_from_cmdline("root=/dev/mtdblock7"), None);
    }

    #[test]
    fn compatible() {
        let c = b"digium,d70\0brcm,bcm11109\0brcm,bcm11107\0";
        assert_eq!(model_from_compatible(c), Some(Model::D70));
        assert_eq!(model_from_compatible(b"brcm,bcm11107\0"), None);
    }

    #[test]
    fn decision() {
        assert_eq!(
            decide(Some(Model::D60), Some(Model::D60)),
            Some(Detected { model: Model::D60, cross_checked: true })
        );
        let d = decide(Some(Model::D60), Some(Model::D62)).unwrap();
        assert_eq!((d.model, d.cross_checked), (Model::D60, false));
        assert_eq!(decide(None, None), None);
        assert_eq!(Model::D40.family(), Family::Old);
        assert_eq!(Model::D65.family(), Family::New);
    }

    #[test]
    fn the_config_and_state_crates_agree_on_every_model() {
        for model in Model::ALL {
            let config = opendeskphone_config::Model::from(model);
            let state = opendeskphone_state::Model::from(model);
            assert_eq!((config.name(), state.name()), (model.name(), model.name()));
            assert_eq!(config.max_accounts(), usize::from(state.max_accounts()), "{model}");
            assert_eq!(config.has_themes(), state.is_color(), "{model}");
        }
    }
}
