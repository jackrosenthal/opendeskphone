//! Displays and backlight.

use crate::{Model, Result, Size};

/// Which panel.  Only the D70 has [`Screen::Side`] (113x357 rapid-dial LCD).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Screen {
    Main,
    Side,
}

/// Pixel layout of a framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// 32 bits per pixel, little-endian `B, G, R, x` in memory (DRM
    /// `XR24`).  Color models.
    Xrgb8888,
    /// 8 bits per pixel gray, 0 = black, 255 = white (DRM `R8  `).  The
    /// panels show four levels; the driver uses the top two bits.
    R8,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Xrgb8888 => 4,
            PixelFormat::R8 => 1,
        }
    }
}

/// Geometry of one screen's framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u16,
    pub height: u16,
    /// Bytes per row (may exceed `width * bytes_per_pixel`).
    pub stride: usize,
    pub format: PixelFormat,
}

impl FrameInfo {
    /// The natural layout for a model's screen.
    pub fn for_model(model: Model, screen: Screen) -> Option<FrameInfo> {
        let size = match screen {
            Screen::Main => model.main_screen(),
            Screen::Side => model.side_screen()?,
        };
        // The D80's panel takes 24 bits a pixel (over LVDS), and the stock
        // kernel gives it a framebuffer of 32 (`bpp=32` on its command
        // line).  That it takes `XR24` is not yet seen on the hardware.
        // Its scan-out is upside down to the user: stock rotates it by 180
        // degrees, and so it was seen on the hardware.
        let format = if model.is_color() { PixelFormat::Xrgb8888 } else { PixelFormat::R8 };
        Some(FrameInfo {
            width: size.width,
            height: size.height,
            stride: size.width as usize * format.bytes_per_pixel(),
            format,
        })
    }
    pub const fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
    /// Total bytes of the framebuffer.
    pub const fn len(&self) -> usize {
        self.stride * self.height as usize
    }
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Rect { x, y, width, height }
    }
    /// Clips to a `width` x `height` screen.
    pub fn clip(self, width: u16, height: u16) -> Rect {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Rect {
            x,
            y,
            width: self.width.min(width - x),
            height: self.height.min(height - y),
        }
    }
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A copy of a framebuffer (screen mirror, screenshots, tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub info: FrameInfo,
    pub pixels: Vec<u8>,
}

/// The panels.  Drawing is: get [`Display::frame`], write pixels, then
/// [`Display::flush`] with the damaged rectangles.  Nothing is guaranteed
/// visible before the flush (the SPI panels are pushed on flush; the color
/// panel scans out of the buffer directly).
///
/// A panel starts off, and opening the display leaves it off: nothing is
/// shown until [`Display::set_power`] turns it on, which shows the frame as
/// it is at that moment.  So the first picture is drawn before the panel is
/// turned on, and the panel never shows an empty or stale buffer.
pub trait Display: Send {
    /// The screens this model has.
    fn screens(&self) -> Vec<Screen>;
    /// Geometry, or `None` if this model lacks that screen.
    fn info(&self, screen: Screen) -> Option<FrameInfo>;
    /// The pixel memory to draw into, [`FrameInfo::len`] bytes.
    fn frame(&mut self, screen: Screen) -> Result<&mut [u8]>;
    /// Makes the damaged areas visible.  An empty slice means the whole
    /// screen.
    fn flush(&mut self, screen: Screen, damage: &[Rect]) -> Result<()>;
    /// Copies out what was last flushed (or the current buffer).
    fn capture(&self, screen: Screen) -> Result<Frame>;
    /// Turns the panel itself on or off.  Turning it on shows the whole
    /// frame as it is now (no flush is needed for it).  On the phone the
    /// kernel switches the panel's backlight with it, at the level last set
    /// through [`Backlight::set_brightness`].
    fn set_power(&mut self, screen: Screen, on: bool) -> Result<()>;
}

/// Highest level of the stock `brightness` and `backlight_dim_level`
/// settings.
pub const BRIGHTNESS_SETTING_MAX: u8 = 10;

/// The stock table from a `brightness` setting (0..=10) to the raw
/// backlight level of a screen.
///
/// The stock phone application (`lcd_main_set_brightness` in
/// `/app/bin/phone`) clamps the setting to 10, indexes an eleven-entry
/// table, and hands the entry to the driver as the raw level; it uses the
/// same table with `backlight_dim_level` when the backlight dims.  The
/// D70's side display has a table of its own, which is not monotonic at
/// the top (17, then 16); it is kept as stock has it.
const fn backlight_table(model: Model, screen: Screen) -> [u8; 11] {
    match (model, screen) {
        (Model::D40 | Model::D45 | Model::D50, _) => [0, 1, 3, 5, 6, 8, 9, 11, 12, 14, 15],
        (Model::D70, Screen::Main) => [0, 1, 4, 6, 9, 11, 13, 15, 17, 19, 21],
        (Model::D70, Screen::Side) => [0, 1, 3, 4, 6, 7, 9, 12, 14, 17, 16],
        (Model::D60 | Model::D62 | Model::D65, _) => [0, 2, 6, 10, 14, 17, 20, 23, 26, 29, 32],
        // Not stock's, which is not yet known (the D80's phone application
        // is an Android one): even steps of its backlight's range, 0..=248.
        (Model::D80, _) => [0, 25, 50, 74, 99, 124, 149, 174, 198, 223, 248],
    }
}

/// The raw backlight level stock uses for a `brightness` (or
/// `backlight_dim_level`) setting of 0..=10; larger settings count as 10.
pub const fn backlight_level(model: Model, screen: Screen, setting: u8) -> u8 {
    let setting = if setting > BRIGHTNESS_SETTING_MAX { BRIGHTNESS_SETTING_MAX } else { setting };
    backlight_table(model, screen)[setting as usize]
}

/// The setting whose stock level is nearest to a raw backlight level (the
/// lowest such setting when two are equally near).
pub const fn backlight_setting(model: Model, screen: Screen, raw: u8) -> u8 {
    let table = backlight_table(model, screen);
    let mut best = 0;
    let mut i = 1;
    while i < table.len() {
        if table[i].abs_diff(raw) < table[best].abs_diff(raw) {
            best = i;
        }
        i += 1;
    }
    best as u8
}

/// Backlight brightness and (gray panels) contrast.
pub trait Backlight: Send {
    /// Highest brightness level (32 color, 31 gray, 248 on the D80); 0 is
    /// off.
    fn max_brightness(&self, screen: Screen) -> u8;
    fn brightness(&self, screen: Screen) -> Result<u8>;
    /// Sets the level.  It lights the backlight only while the panel is on
    /// ([`Display::set_power`]); set while the panel is off, it is the
    /// level the backlight comes on with.
    fn set_brightness(&mut self, screen: Screen, level: u8) -> Result<()>;
    /// Highest contrast level: 10 (the scale of the stock `contrast`
    /// setting, 0..=10), or 0 when the panel has no contrast control.
    fn max_contrast(&self, screen: Screen) -> u8;
    fn contrast(&self, screen: Screen) -> Result<u8>;
    fn set_contrast(&mut self, screen: Screen, level: u8) -> Result<()>;
}

#[cfg(test)]
mod backlight_tests {
    use super::*;

    /// The tables as read from `/app/bin/phone` of stock 2_9_26 (D40, D45,
    /// D50, D70) and 2_9_28 (D60, D62, D65).
    #[test]
    fn levels_follow_the_stock_tables() {
        let levels = |m, s| (0..=10).map(|b| backlight_level(m, s, b)).collect::<Vec<u8>>();
        for m in [Model::D40, Model::D45, Model::D50] {
            assert_eq!(levels(m, Screen::Main), [0, 1, 3, 5, 6, 8, 9, 11, 12, 14, 15]);
        }
        assert_eq!(levels(Model::D70, Screen::Main), [0, 1, 4, 6, 9, 11, 13, 15, 17, 19, 21]);
        assert_eq!(levels(Model::D70, Screen::Side), [0, 1, 3, 4, 6, 7, 9, 12, 14, 17, 16]);
        for m in [Model::D60, Model::D62, Model::D65] {
            assert_eq!(levels(m, Screen::Main), [0, 2, 6, 10, 14, 17, 20, 23, 26, 29, 32]);
        }
        // Above the scale is the top of the scale.
        assert_eq!(backlight_level(Model::D40, Screen::Main, 200), 15);
    }

    #[test]
    fn no_level_exceeds_the_driver_range() {
        for m in Model::ALL {
            for s in [Screen::Main, Screen::Side] {
                for b in 0..=10 {
                    assert!(backlight_level(m, s, b) <= m.backlight_max());
                }
                assert_eq!(backlight_level(m, s, 0), 0);
            }
        }
    }

    #[test]
    fn settings_read_back() {
        for m in [Model::D40, Model::D60, Model::D70] {
            for b in 0..=10 {
                assert_eq!(backlight_setting(m, Screen::Main, backlight_level(m, Screen::Main, b)), b);
            }
            assert_eq!(backlight_setting(m, Screen::Main, 255), 10);
        }
        // Between two levels the nearer one wins; a tie goes to the lower.
        assert_eq!(backlight_setting(Model::D40, Screen::Main, 7), 4);
        assert_eq!(backlight_setting(Model::D40, Screen::Main, 31), 10);
        assert_eq!(backlight_setting(Model::D60, Screen::Main, 16), 5);
        // The side display's table falls at the top: 17 is setting 9.
        assert_eq!(backlight_setting(Model::D70, Screen::Side, 17), 9);
        assert_eq!(backlight_setting(Model::D70, Screen::Side, 16), 10);
    }
}
