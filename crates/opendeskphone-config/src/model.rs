//! This module describes the phone models to the configuration.

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

impl Model {
    pub const ALL: [Model; 8] =
        [Model::D40, Model::D45, Model::D50, Model::D60, Model::D62, Model::D65, Model::D70, Model::D80];

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

    /// SIP account limit set by the factory layer.
    pub fn max_accounts(self) -> usize {
        match self {
            Model::D40 | Model::D45 | Model::D60 | Model::D62 => 2,
            Model::D50 => 4,
            Model::D65 | Model::D70 | Model::D80 => 6,
        }
    }

    /// The color models' screens show themes and wallpapers.
    pub fn has_themes(self) -> bool {
        matches!(self, Model::D60 | Model::D62 | Model::D65 | Model::D80)
    }

    /// The compiled-in factory layer for this model.
    pub fn factory_config(self) -> &'static str {
        match self {
            Model::D40 => include_str!("../defaults/factory-D40.xml"),
            Model::D45 => include_str!("../defaults/factory-D45.xml"),
            Model::D50 => include_str!("../defaults/factory-D50.xml"),
            Model::D60 => include_str!("../defaults/factory-D60.xml"),
            Model::D62 => include_str!("../defaults/factory-D62.xml"),
            Model::D65 => include_str!("../defaults/factory-D65.xml"),
            Model::D70 => include_str!("../defaults/factory-D70.xml"),
            Model::D80 => include_str!("../defaults/factory-D80.xml"),
        }
    }
}

/// The compiled-in model-independent defaults.
pub const BASE_CONFIG: &str = include_str!("../defaults/base-config.xml");

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Model {
    type Err = ();
    /// Accepts the `digium_board=` spelling, in any case.
    fn from_str(s: &str) -> Result<Self, ()> {
        Model::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(s.trim())).ok_or(())
    }
}
