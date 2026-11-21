//! Backlight, contrast, and lamps through sysfs class devices.

use super::sys::*;
use crate::*;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Name the gray-LCD driver gives the D70 side display's backlight.
const SIDE_BACKLIGHT: &str = "digium-dss";

fn class_entries(class: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(Path::new("/sys/class").join(class))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn file_name(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// Backlight class and LCD class (contrast) devices.
pub struct SysBacklight {
    model: Model,
    main_bl: Option<PathBuf>,
    side_bl: Option<PathBuf>,
    main_lcd: Option<PathBuf>,
    side_lcd: Option<PathBuf>,
}

/// Highest contrast level: the stock `contrast` setting runs 0..=10.
pub const CONTRAST_LEVELS: u8 = 10;

/// First raw value and step of the stock level-to-value contrast table.
///
/// The stock phone application (`lcd_set_contrast` in `/app/bin/phone`)
/// indexes an eleven-entry table with the `contrast` setting and hands the
/// entry to the panel driver as the raw controller value: 240, 242, ... 260
/// on the D40, D45, and D50 (of 0..=511), 324, 328, ... 364 on the D70's
/// main panel, and 120, 122, ... 140 on the D70's side display (of
/// 0..=255).  The usable window is that narrow: far below it the glass
/// shows nothing at all, far above it everything is dark.
const fn contrast_table(model: Model, screen: Screen) -> (u32, u32) {
    match (model, screen) {
        (Model::D70, Screen::Side) => (120, 2),
        (Model::D70, Screen::Main) => (324, 4),
        _ => (240, 2),
    }
}

/// The raw controller value for a contrast level (0..=10).
pub const fn contrast_raw(model: Model, screen: Screen, level: u8) -> u32 {
    let (first, step) = contrast_table(model, screen);
    let level = if level > CONTRAST_LEVELS { CONTRAST_LEVELS } else { level };
    first + step * level as u32
}

/// The contrast level nearest to a raw controller value.
pub const fn contrast_level(model: Model, screen: Screen, raw: u32) -> u8 {
    let (first, step) = contrast_table(model, screen);
    let level = (raw.saturating_sub(first) + step / 2) / step;
    if level > CONTRAST_LEVELS as u32 { CONTRAST_LEVELS } else { level as u8 }
}

impl SysBacklight {
    pub fn open(model: Model) -> Result<SysBacklight> {
        let bls = class_entries("backlight");
        let lcds = class_entries("lcd");
        let b = SysBacklight {
            model,
            main_bl: bls.iter().find(|p| file_name(p) != SIDE_BACKLIGHT).cloned(),
            side_bl: bls.iter().find(|p| file_name(p) == SIDE_BACKLIGHT).cloned(),
            main_lcd: lcds.iter().find(|p| !file_name(p).ends_with("-dss")).cloned(),
            side_lcd: lcds.iter().find(|p| file_name(p).ends_with("-dss")).cloned(),
        };
        if b.main_bl.is_none() {
            return Err(HalError::Unavailable("no backlight class device".into()));
        }
        Ok(b)
    }

    fn bl(&self, screen: Screen) -> Result<&Path> {
        match screen {
            Screen::Main => self.main_bl.as_deref(),
            Screen::Side => self.side_bl.as_deref(),
        }
        .ok_or(HalError::Unsupported("no backlight for that screen"))
    }

    fn lcd(&self, screen: Screen) -> Result<&Path> {
        match screen {
            Screen::Main => self.main_lcd.as_deref(),
            Screen::Side => self.side_lcd.as_deref(),
        }
        .ok_or(HalError::Unsupported("no contrast control for that screen"))
    }
}

fn write_value(path: &Path, v: u32) -> Result<()> {
    std::fs::write(path, v.to_string()).map_err(|e| HalError::io(path.display().to_string(), e))
}

impl Backlight for SysBacklight {
    fn max_brightness(&self, screen: Screen) -> u8 {
        match self.bl(screen) {
            Ok(p) => read_u32(&p.join("max_brightness")).map(|v| v.min(255) as u8).unwrap_or(0),
            Err(_) => 0,
        }
    }

    fn brightness(&self, screen: Screen) -> Result<u8> {
        let p = self.bl(screen)?.join("brightness");
        read_u32(&p).map(|v| v.min(255) as u8).map_err(|e| HalError::io(p.display().to_string(), e))
    }

    fn set_brightness(&mut self, screen: Screen, level: u8) -> Result<()> {
        let dir = self.bl(screen)?;
        let max = self.max_brightness(screen);
        let level = level.min(max);
        // Only the level is written, never `bl_power`: the kernel switches
        // a backlight on and off with its panel (the panel bridge of the
        // color panel and the gray driver both call `backlight_enable` when
        // the CRTC is enabled and `backlight_disable` when it is disabled),
        // so a level set while the panel is off takes effect when the panel
        // comes on, and forcing the power on here would light a panel that
        // shows nothing yet.  Level 0 is dark in both backlight drivers.
        write_value(&dir.join("brightness"), level as u32)
    }

    /// Contrast is the stock setting's level (0..=10), mapped to the raw
    /// controller value by the stock tables (see [`contrast_raw`]).
    fn max_contrast(&self, screen: Screen) -> u8 {
        if self.lcd(screen).is_ok() { CONTRAST_LEVELS } else { 0 }
    }

    fn contrast(&self, screen: Screen) -> Result<u8> {
        let p = self.lcd(screen)?.join("contrast");
        let raw = read_u32(&p).map_err(|e| HalError::io(p.display().to_string(), e))?;
        Ok(contrast_level(self.model, screen, raw))
    }

    fn set_contrast(&mut self, screen: Screen, level: u8) -> Result<()> {
        let raw = contrast_raw(self.model, screen, level);
        write_value(&self.lcd(screen)?.join("contrast"), raw)
    }
}

impl SysBacklight {
    /// The model this was opened for.
    pub fn model(&self) -> Model {
        self.model
    }
}

/// Parses an LED class name of the port's devicetrees:
/// `<color>:<function>[-<n>]`, e.g., `green:line-0`, `red:mail`.
///
/// The D80 has one lamp, a red and a green LED that its devicetree names
/// `status`.  It is taken for the message lamp, the one lamp of the other
/// models that is not a key's: what stock shows on it is not known.
pub(crate) fn parse_led_name(name: &str) -> Option<(Lamp, bool)> {
    let (color, func) = name.split_once(':')?;
    let red = match color {
        "red" => true,
        "green" => false,
        _ => return None,
    };
    let (func, n) = match func.rsplit_once('-') {
        Some((f, n)) if n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty() => {
            (f, n.parse::<u8>().ok())
        }
        _ => (func, None),
    };
    let lamp = match (func, n) {
        ("line", Some(n)) => Lamp::Line(n),
        ("blf", Some(n)) => Lamp::Blf(n),
        ("mail" | "status", _) => Lamp::Message,
        ("mute", _) => Lamp::Mute,
        ("headset", _) => Lamp::Headset,
        ("speaker", _) => Lamp::Speaker,
        _ => return None,
    };
    Some((lamp, red))
}

struct SysLamp {
    lamp: Lamp,
    red: Option<File>,
    green: Option<File>,
}

/// LED class lamps.
pub struct SysLamps {
    lamps: Vec<SysLamp>,
}

impl SysLamps {
    pub fn open() -> Result<SysLamps> {
        let mut lamps: Vec<SysLamp> = Vec::new();
        for dir in class_entries("leds") {
            let Some((lamp, red)) = parse_led_name(file_name(&dir)) else { continue };
            let Ok(f) = std::fs::OpenOptions::new().write(true).open(dir.join("brightness")) else {
                continue;
            };
            let idx = match lamps.iter().position(|l| l.lamp == lamp) {
                Some(i) => i,
                None => {
                    lamps.push(SysLamp { lamp, red: None, green: None });
                    lamps.len() - 1
                }
            };
            if red {
                lamps[idx].red = Some(f);
            } else {
                lamps[idx].green = Some(f);
            }
        }
        if lamps.is_empty() {
            return Err(HalError::Unavailable("no phone lamps under /sys/class/leds".into()));
        }
        lamps.sort_by_key(|l| l.lamp);
        Ok(SysLamps { lamps })
    }
}

fn led_write(f: &mut Option<File>, on: bool) -> Result<()> {
    if let Some(f) = f {
        // The LED core clamps to max_brightness; these lamps are on/off.
        f.write_all(if on { b"255" } else { b"0" }).map_err(|e| HalError::io("LED brightness", e))?;
    }
    Ok(())
}

impl Lamps for SysLamps {
    fn lamps(&self) -> Vec<Lamp> {
        self.lamps.iter().map(|l| l.lamp).collect()
    }

    fn set(&mut self, lamp: Lamp, color: LampColor) -> Result<()> {
        let Some(l) = self.lamps.iter_mut().find(|l| l.lamp == lamp) else { return Ok(()) };
        let (red, green) = match color {
            LampColor::Off => (false, false),
            LampColor::Red => (true, false),
            LampColor::Green => (false, true),
            LampColor::Amber => (true, true),
        };
        led_write(&mut l.red, red)?;
        led_write(&mut l.green, green)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock tables, as read from `/app/bin/phone` of 2_9_26.
    #[test]
    fn contrast_levels_follow_the_stock_tables() {
        let small: Vec<u32> = (0..=10).map(|l| contrast_raw(Model::D40, Screen::Main, l)).collect();
        assert_eq!(small, [240, 242, 244, 246, 248, 250, 252, 254, 256, 258, 260]);
        for m in [Model::D45, Model::D50] {
            assert_eq!(contrast_raw(m, Screen::Main, 5), 250);
        }
        let large: Vec<u32> = (0..=10).map(|l| contrast_raw(Model::D70, Screen::Main, l)).collect();
        assert_eq!(large, [324, 328, 332, 336, 340, 344, 348, 352, 356, 360, 364]);
        let side: Vec<u32> = (0..=10).map(|l| contrast_raw(Model::D70, Screen::Side, l)).collect();
        assert_eq!(side, [120, 122, 124, 126, 128, 130, 132, 134, 136, 138, 140]);
        // Above the scale is the top of the scale, never a larger raw value.
        assert_eq!(contrast_raw(Model::D40, Screen::Main, 200), 260);
    }

    #[test]
    fn contrast_levels_read_back() {
        for (m, s) in [(Model::D40, Screen::Main), (Model::D70, Screen::Main), (Model::D70, Screen::Side)] {
            for l in 0..=10 {
                assert_eq!(contrast_level(m, s, contrast_raw(m, s, l)), l);
            }
            assert_eq!(contrast_level(m, s, 0), 0);
            assert_eq!(contrast_level(m, s, 511), 10);
        }
        // The driver's power-on values: the top of the small scale, and the
        // middle of the D70's two.
        assert_eq!(contrast_level(Model::D40, Screen::Main, 260), 10);
        assert_eq!(contrast_level(Model::D70, Screen::Main, 344), 5);
        assert_eq!(contrast_level(Model::D70, Screen::Side, 130), 5);
    }

    #[test]
    fn led_names() {
        assert_eq!(parse_led_name("green:line-0"), Some((Lamp::Line(0), false)));
        assert_eq!(parse_led_name("red:line-5"), Some((Lamp::Line(5), true)));
        assert_eq!(parse_led_name("red:blf-9"), Some((Lamp::Blf(9), true)));
        assert_eq!(parse_led_name("green:mail"), Some((Lamp::Message, false)));
        assert_eq!(parse_led_name("red:mute"), Some((Lamp::Mute, true)));
        assert_eq!(parse_led_name("green:headset"), Some((Lamp::Headset, false)));
        assert_eq!(parse_led_name("red:speaker"), Some((Lamp::Speaker, true)));
        assert_eq!(parse_led_name("red:status"), Some((Lamp::Message, true)));
        assert_eq!(parse_led_name("green:status"), Some((Lamp::Message, false)));
        assert_eq!(parse_led_name("mmc0::"), None);
        assert_eq!(parse_led_name("blue:line-0"), None);
        assert_eq!(parse_led_name("green:line"), None);
    }
}
