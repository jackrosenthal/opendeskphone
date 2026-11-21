//! Diagnostics: the display, key, lamp, and tone tests, the state of the
//! phone's subsystems, and its log.

use opendeskphone_app_sdk::canvas::{self, Bitmap, Color, Op, Role, Text};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::diagnostics::{HealthState, Subsystem};
use opendeskphone_app_sdk::{Align, Font, diagnostics, input, lamps};

const SK_SELECT: u32 = 1;
const SK_BACK: u32 = 4;
/// A softkey position left empty.
const SK_NONE: u32 = 0;

const ITEMS: [&str; 6] = [
    "Display test",
    "Key test",
    "Lamp test",
    "Tone test",
    "Subsystems",
    "Log",
];
/// The place of the key test in `ITEMS`.
const ITEM_KEYS: usize = 1;

/// The items of the menu on this phone, as their places in `ITEMS`: a
/// phone without physical buttons has no key test.
fn menu_items() -> Vec<usize> {
    let keys = ui::get_metrics().keys;
    (0..ITEMS.len()).filter(|i| keys || *i != ITEM_KEYS).collect()
}
const LOG_ENTRIES: u32 = 40;
/// How often a status page is brought up to date.
const REFRESH_MS: u32 = 1000;

/// The patterns of the display test, in order.
const PATTERNS: u8 = 6;
/// The line of the last pattern, in every font.
const SAMPLE: &str = "Open Desk Phone 0123456789 AaBbGgQq";
/// The key test shows this many of the newest keys.
const KEYS_SHOWN: usize = 24;
/// How long every lamp shows a color.
const LAMP_STEP_MS: u32 = 500;
const LAMP_COLORS: [lamps::Color; 4] = [
    lamps::Color::Red,
    lamps::Color::Green,
    lamps::Color::Amber,
    lamps::Color::Off,
];

/// The test tone: the dial tone (350 Hz and 440 Hz, each at -13 dBm0) for
/// two seconds.
const TONE_HZ: [usize; 2] = [350, 440];
const TONE_AMPLITUDE: f32 = 5110.0;
const TONE_SECONDS: usize = 2;
/// The sample rate of a headerless sound.
const SAMPLE_RATE: usize = 16_000;
/// The tone repeats after this many samples: both frequencies are whole
/// multiples of 10 Hz.
const TONE_PERIOD: usize = SAMPLE_RATE / 10;

const BLACK: Color = gray(0x00);
const DARK_GRAY: Color = gray(0x55);
const WHITE: Color = gray(0xFF);

const fn gray(level: u8) -> Color {
    Color {
        r: level,
        g: level,
        b: level,
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b }
}

#[derive(Clone, Copy, PartialEq)]
enum Info {
    Health,
    Log,
}

impl Info {
    fn title(self) -> &'static str {
        match self {
            Info::Health => "Subsystems",
            Info::Log => "Log",
        }
    }
    /// The page's rows: a label (may be empty) and a value.
    fn rows(self) -> Vec<(String, String)> {
        match self {
            Info::Health => diagnostics::health()
                .into_iter()
                .map(|h| (subsystem_label(h.subsystem).to_string(), health_text(&h.health)))
                .collect(),
            Info::Log => diagnostics::log_tail(LOG_ENTRIES)
                .into_iter()
                .map(|line| (String::new(), line))
                .collect(),
        }
    }
}

fn subsystem_label(s: Subsystem) -> &'static str {
    match s {
        Subsystem::Core => "Core",
        Subsystem::Configuration => "Configuration",
        Subsystem::Network => "Network",
        Subsystem::Sip => "SIP",
        Subsystem::Media => "Media",
        Subsystem::Dpma => "DPMA",
        Subsystem::Ui => "UI",
        Subsystem::Apps => "Apps",
        Subsystem::Web => "Web",
        Subsystem::Ssh => "SSH",
        Subsystem::Update => "Update",
    }
}

fn health_text(h: &HealthState) -> String {
    match h {
        HealthState::Ok => "ok".to_string(),
        HealthState::Starting => "starting".to_string(),
        HealthState::Stopped => "stopped".to_string(),
        HealthState::Degraded(why) => format!("degraded: {why}"),
        HealthState::Failed(why) => format!("failed: {why}"),
    }
}

/// The name the key test shows for a key: the phone's key vocabulary.
fn key_name(k: Key) -> String {
    match k {
        Key::Digit(d) => d.to_string(),
        Key::Star => "*".into(),
        Key::Pound => "#".into(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Select => "select".into(),
        Key::Cancel => "cancel".into(),
        Key::VolumeUp => "vup".into(),
        Key::VolumeDown => "vdn".into(),
        Key::Softkey(n) => format!("sk{n}"),
        Key::Line(n) => format!("line{n}"),
        Key::Blf(n) => format!("blf{n}"),
        Key::Mute => "mute".into(),
        Key::Speaker => "spkr".into(),
        Key::Headset => "hdst".into(),
        Key::Hold => "hold".into(),
        Key::Transfer => "xfer".into(),
        Key::Redial => "redial".into(),
        Key::Messages => "msgs".into(),
        Key::Conference => "conf".into(),
        Key::Info => "info".into(),
        Key::BlfPageLeft => "blfleft".into(),
        Key::BlfPageRight => "blfright".into(),
        Key::Status => "status".into(),
        Key::Contacts => "dir".into(),
        Key::Apps => "apps".into(),
        Key::Menu => "menu".into(),
        Key::Expansion(e) => format!("exp{}.{}", e.module, e.key),
        Key::Other(code) => format!("key{code}"),
    }
}

#[derive(Default)]
enum Page {
    #[default]
    None,
    Menu {
        list: Widget,
    },
    Info {
        which: Info,
        /// The value widget of each row.
        values: Vec<Widget>,
        rows: Vec<(String, String)>,
    },
    /// Full-screen patterns; any key shows the next one.
    Display {
        pattern: u8,
    },
    /// The name of every key and hook change, as it happens.
    Keys {
        text: Widget,
        /// The newest names, oldest first.
        names: Vec<String>,
        /// Presses of Cancel in a row.
        cancels: u8,
    },
    /// Every lamp cycles through its colors.
    Lamps {
        step: usize,
    },
}

#[derive(Default)]
struct Diagnostics {
    page: Page,
    /// The menu row to come back to.
    selected: u32,
    timer: Option<u32>,
}

fn blank() -> Softkey {
    Softkey::new(SK_NONE, "")
}

/// The space left and right of text.
fn inset(m: &opendeskphone_app_sdk::Metrics) -> u16 {
    match m.skin {
        Skin::Touch => 24,
        _ if m.color => 15,
        _ => 4,
    }
}

/// The softkeys of a page: on a phone with physical buttons, `keys`; on the
/// touchscreen none, where a row is touched and the back button goes
/// back.
fn page_softkeys(keys: &[Softkey]) {
    if ui::get_metrics().keys {
        ui::set_softkeys(ROOT, keys);
    }
}

/// A rectangle's four edges, one pixel wide.
fn frame(ops: &mut Vec<Op>, x: i16, y: i16, w: u16, h: u16, color: Color) {
    let (right, bottom) = (x + w as i16 - 1, y + h as i16 - 1);
    ops.push(Op::Fill((Rect::new(x, y, w, 1), color)));
    ops.push(Op::Fill((Rect::new(x, bottom, w, 1), color)));
    ops.push(Op::Fill((Rect::new(x, y, 1, h), color)));
    ops.push(Op::Fill((Rect::new(right, y, 1, h), color)));
}

/// Every other pixel of every other row, over the whole display: a
/// bitmap is at most 256 pixels each way, so the display takes several.
/// A display with more pixels than a drawing's bitmaps may have (64 KiB:
/// the touchscreen) gets squares of two pixels.
fn checkerboard(ops: &mut Vec<Op>, w: u16, h: u16) {
    const TILE: u16 = 256;
    let scale: u16 = if (w as usize).div_ceil(8) * h as usize > 64 << 10 { 2 } else { 1 };
    let (w, h) = (w.div_ceil(scale), h.div_ceil(scale));
    for top in (0..h).step_by(TILE as usize) {
        for left in (0..w).step_by(TILE as usize) {
            let (width, height) = ((w - left).min(TILE), (h - top).min(TILE));
            let row_bytes = (width as usize).div_ceil(8);
            let mut row = vec![0xAA_u8; row_bytes];
            if width % 8 != 0 {
                row[row_bytes - 1] &= 0xFF << (8 - width % 8);
            }
            let mut bits = Vec::with_capacity(row_bytes * height as usize);
            for y in 0..height {
                if y % 2 == 0 {
                    bits.extend_from_slice(&row);
                } else {
                    bits.resize(bits.len() + row_bytes, 0);
                }
            }
            ops.push(Op::Bitmap(Bitmap {
                x: (left * scale) as i16,
                y: (top * scale) as i16,
                width,
                height,
                scale: scale as u8,
                color: BLACK,
                bits,
            }));
        }
    }
}

/// One pattern of the display test, for a display of this size.
fn pattern(which: u8, w: u16, h: u16, color: bool) -> Vec<Op> {
    let all = Rect::new(0, 0, w, h);
    let mut ops = Vec::new();
    match which {
        0 => ops.push(Op::Fill((all, WHITE))),
        1 => ops.push(Op::Fill((all, BLACK))),
        2 => {
            // Color bars, or the four grays.
            let bars: &[Color] = if color {
                &[
                    rgb(255, 0, 0),
                    rgb(0, 255, 0),
                    rgb(0, 0, 255),
                    rgb(255, 255, 0),
                    rgb(0, 255, 255),
                    rgb(255, 0, 255),
                    WHITE,
                    BLACK,
                ]
            } else {
                &[BLACK, DARK_GRAY, gray(0xAA), WHITE]
            };
            let bar = w / bars.len() as u16;
            for (i, c) in bars.iter().enumerate() {
                ops.push(Op::Fill((Rect::new((i as u16 * bar) as i16, 0, bar + 1, h), *c)));
            }
        }
        3 => {
            // Single pixels: a stuck column or row shows.
            ops.push(Op::Fill((all, WHITE)));
            checkerboard(&mut ops, w, h);
        }
        4 => {
            // The edges of the display, and its middle.
            ops.push(Op::Fill((all, WHITE)));
            frame(&mut ops, 0, 0, w, h, BLACK);
            frame(&mut ops, 4, 4, w - 8, h - 8, DARK_GRAY);
            ops.push(Op::Fill((Rect::new(0, (h / 2) as i16, w, 1), BLACK)));
            ops.push(Op::Fill((Rect::new((w / 2) as i16, 0, 1, h), BLACK)));
        }
        _ => {
            // Every font of the skin.
            ops.push(Op::Fill((all, canvas::theme_color(Role::Background))));
            let fg = canvas::theme_color(Role::Foreground);
            // (Touch: below the back button.)
            let m = ui::get_metrics();
            let mut y = if m.keys { 4 } else { m.title_height as i16 };
            for font in [Font::Small, Font::Normal, Font::Bold, Font::Title, Font::Large] {
                let size = canvas::text_size(font, SAMPLE);
                ops.push(Op::Text(Text {
                    box_: Rect::new(4, y, size.width, size.height),
                    font,
                    color: fg,
                    text: SAMPLE.to_string(),
                }));
                y += (size.height + size.height / 2) as i16;
            }
        }
    }
    ops
}

/// The test tone as a headerless sound: 16-bit little-endian samples.
/// One period is worked out, and repeated.
fn tone() -> Vec<u8> {
    let step = TONE_HZ.map(|hz| core::f32::consts::TAU * hz as f32 / SAMPLE_RATE as f32);
    let mut period = Vec::with_capacity(TONE_PERIOD * 2);
    for i in 0..TONE_PERIOD {
        let t = i as f32;
        let sample = TONE_AMPLITUDE * ((t * step[0]).sin() + (t * step[1]).sin());
        period.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    period.repeat(SAMPLE_RATE * TONE_SECONDS / TONE_PERIOD)
}

impl Diagnostics {
    fn stop_timer(&mut self) {
        if let Some(t) = self.timer.take() {
            timers::cancel(t);
        }
    }

    /// Gives back what a test took: the whole display, the keys, and the
    /// lamps.
    fn end_test(&mut self) {
        self.stop_timer();
        match self.page {
            Page::Display { .. } => {
                input::capture(false);
                ui::set_full_screen(false);
            }
            Page::Keys { .. } => input::capture(false),
            Page::Lamps { .. } => lamps::release(),
            _ => {}
        }
    }

    fn show_menu(&mut self) {
        self.end_test();
        let r = screen("Diagnostics");
        // As in the phone's own menus: Right selects, and a digit selects
        // the row of that number.
        let mut claims = vec![Key::Cancel, Key::Left, Key::Right];
        claims.extend((1..=6).map(Key::Digit));
        ui::claim_keys(ROOT, &claims);
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = menu_items().into_iter().map(|i| ListItem::new(ITEMS[i])).collect();
        ui::set_items(list, &items);
        ui::set_selected(list, self.selected);
        page_softkeys(&[
            Softkey::new(SK_SELECT, "Select"),
            blank(),
            blank(),
            Softkey::new(SK_BACK, "Back"),
        ]);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.page = Page::Menu { list };
    }

    fn show_info(&mut self, which: Info) {
        let rows = which.rows();
        let r = screen(which.title());
        ui::claim_keys(ROOT, &[Key::Cancel, Key::Left, Key::Select]);
        let scroll = ui::create(WidgetKind::Scroll, ROOT, r);
        let m = ui::get_metrics();
        let inset = inset(&m);
        let width = r.width.saturating_sub(2 * inset + m.scroll_bar_width);
        let mut values = Vec::with_capacity(rows.len());
        for (i, (label, value)) in rows.iter().enumerate() {
            let y = (i as u16 * m.row_height) as i16;
            let label_w = if label.is_empty() { 0 } else { width * 2 / 5 };
            if label_w > 0 {
                let l = ui::create(
                    WidgetKind::Text,
                    scroll,
                    Rect::new(inset as i16, y, label_w, m.row_height),
                );
                ui::set_text(l, label);
            }
            let v = ui::create(
                WidgetKind::Text,
                scroll,
                Rect::new((inset + label_w) as i16, y, width - label_w, m.row_height),
            );
            if label_w > 0 {
                ui::set_align(v, Align::Right);
            }
            ui::set_text(v, value);
            values.push(v);
        }
        page_softkeys(&[blank(), blank(), blank(), Softkey::new(SK_BACK, "Back")]);
        ui::set_focus(scroll);
        self.page = Page::Info {
            which,
            values,
            rows,
        };
        if self.timer.is_none() {
            self.timer = timers::set(REFRESH_MS, true).ok();
        }
    }

    /// Brings the status page on screen up to date.
    fn refresh(&mut self) {
        let Page::Info {
            which,
            values,
            rows,
        } = &mut self.page
        else {
            return;
        };
        let now = which.rows();
        if now == *rows {
            return;
        }
        if now.len() != rows.len() {
            let which = *which;
            self.show_info(which);
            return;
        }
        for ((old, new), w) in rows.iter().zip(&now).zip(values.iter()) {
            if old.1 != new.1 {
                ui::set_text(*w, &new.1);
            }
        }
        *rows = now;
    }

    /// A pattern of the display test on the whole display.  Every key is
    /// the test's, so that any key shows the next pattern; on the
    /// touchscreen a touch anywhere does, and the back button, which is
    /// over the pattern's corner, leaves.
    fn show_display(&mut self, pattern_index: u8) {
        ui::clear(ROOT);
        ui::set_full_screen(true);
        input::capture(true);
        ui::claim_touch(ROOT, true);
        let m = ui::get_metrics();
        let picture = canvas::create(ROOT, Rect::new(0, 0, m.width, m.height));
        if let Err(e) = canvas::draw(picture, &pattern(pattern_index, m.width, m.height, m.color)) {
            opendeskphone_app_sdk::log_warn(&format!("the display test could not draw: {e}"));
        }
        self.page = Page::Display {
            pattern: pattern_index,
        };
    }

    /// The key test, showing `names`.
    fn show_keys(&mut self, names: Vec<String>) {
        let r = screen("Key test");
        input::capture(true);
        let m = ui::get_metrics();
        let inset = inset(&m);
        let width = r.width.saturating_sub(2 * inset);
        let hint = ui::create(
            WidgetKind::Text,
            ROOT,
            Rect::new(inset as i16, r.y, width, m.line_height),
        );
        ui::set_font(hint, Font::Small);
        ui::set_text(hint, "Cancel twice to exit");
        let text = ui::create(
            WidgetKind::Text,
            ROOT,
            Rect::new(
                inset as i16,
                r.y + m.line_height as i16,
                width,
                r.height.saturating_sub(m.line_height),
            ),
        );
        ui::set_wrap(text, true);
        ui::set_text(text, &names.join(" "));
        self.page = Page::Keys {
            text,
            names,
            cancels: 0,
        };
    }

    /// A key (`key`) or a hook change in the key test.  Cancel twice in a
    /// row leaves the test.
    fn key_tested(&mut self, name: String, cancel: Option<bool>) {
        let Page::Keys {
            text,
            names,
            cancels,
        } = &mut self.page
        else {
            return;
        };
        if let Some(cancel) = cancel {
            *cancels = if cancel { *cancels + 1 } else { 0 };
            if *cancels >= 2 {
                return self.show_menu();
            }
        }
        names.push(name);
        if names.len() > KEYS_SHOWN {
            names.remove(0);
        }
        ui::set_text(*text, &names.join(" "));
    }

    /// The lamp test: every lamp shows the color of `step`.
    fn show_lamps(&mut self, step: usize) {
        let m = ui::get_metrics();
        if !m.keys {
            // Touch: the title of every screen, and the line under it.
            let area = screen("Lamp test");
            opendeskphone_app_sdk::nav::note(area, "Cycling all lamps");
            return self.cycle_lamps(step);
        }
        ui::clear(ROOT);
        ui::claim_keys(ROOT, &[Key::Cancel, Key::Select]);
        let inset = inset(&m);
        let width = m.width.saturating_sub(2 * inset);
        let top = (m.height.saturating_sub(3 * m.line_height) / 2) as i16;
        let title = ui::create(
            WidgetKind::Text,
            ROOT,
            Rect::new(inset as i16, top, width, m.line_height),
        );
        ui::set_font(title, Font::Title);
        ui::set_align(title, Align::Center);
        ui::set_text(title, "Lamp test");
        let text = ui::create(
            WidgetKind::Text,
            ROOT,
            Rect::new(inset as i16, top + m.line_height as i16, width, m.line_height),
        );
        ui::set_align(text, Align::Center);
        ui::set_text(text, "Cycling all lamps");
        page_softkeys(&[blank(), blank(), blank(), Softkey::new(SK_BACK, "Back")]);
        self.cycle_lamps(step);
    }

    /// Lights the lamps in the color of `step`, and goes on cycling.
    fn cycle_lamps(&mut self, step: usize) {
        self.light(step);
        if self.timer.is_none() {
            self.timer = timers::set(LAMP_STEP_MS, true).ok();
        }
    }

    fn light(&mut self, step: usize) {
        let color = LAMP_COLORS[step % LAMP_COLORS.len()];
        for lamp in lamps::list() {
            lamps::set(lamp, color);
        }
        self.page = Page::Lamps { step };
    }

    fn activate(&mut self, row: u32) {
        self.selected = row;
        let Some(item) = menu_items().get(row as usize).copied() else {
            return;
        };
        match item {
            0 => self.show_display(0),
            1 => self.show_keys(Vec::new()),
            2 => self.show_lamps(0),
            3 => {
                if let Err(e) = audio::play(&tone(), 0) {
                    opendeskphone_app_sdk::log_warn(&format!("the test tone could not play: {e:?}"));
                }
            }
            4 => self.show_info(Info::Health),
            5 => self.show_info(Info::Log),
            _ => {}
        }
    }

    fn back(&mut self) {
        match self.page {
            Page::Menu { .. } | Page::None => runtime::background(),
            _ => self.show_menu(),
        }
    }

    /// The page the app was on, drawn anew.
    fn show_again(&mut self) {
        match std::mem::take(&mut self.page) {
            Page::Info { which, .. } => self.show_info(which),
            Page::Display { pattern } => self.show_display(pattern),
            Page::Keys { names, .. } => self.show_keys(names),
            Page::Lamps { step } => self.show_lamps(step),
            Page::Menu { .. } | Page::None => self.show_menu(),
        }
    }

    fn key(&mut self, key: Key) {
        match &self.page {
            // Cancel leaves the test, and so does any key after the last
            // pattern.
            Page::Display { pattern } => {
                if key == Key::Cancel || pattern + 1 >= PATTERNS {
                    self.show_menu();
                } else {
                    let next = pattern + 1;
                    self.show_display(next);
                }
            }
            Page::Keys { .. } => self.key_tested(key_name(key), Some(key == Key::Cancel)),
            Page::Menu { list } if key == Key::Right => {
                if let Some(row) = ui::selected(*list) {
                    self.activate(row);
                }
            }
            Page::Menu { list } if matches!(key, Key::Digit(1..=9)) => {
                let Key::Digit(d) = key else { return };
                let row = u32::from(d - 1);
                ui::set_selected(*list, row);
                self.activate(row);
            }
            _ => self.back(),
        }
    }
}

impl App for Diagnostics {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => self.show_again(),
            Event::Background => self.stop_timer(),
            Event::Timer(_) => match self.page {
                Page::Lamps { step } => self.light(step + 1),
                _ => self.refresh(),
            },
            Event::Widget((_, WidgetEvent::Highlighted(row))) => self.selected = row,
            Event::Widget((_, WidgetEvent::Activated(row))) => self.activate(row),
            Event::Softkey(SK_SELECT) => {
                if let Page::Menu { list } = self.page
                    && let Some(row) = ui::selected(list)
                {
                    self.activate(row);
                }
            }
            Event::Softkey(SK_BACK) => self.back(),
            // A touch on a pattern of the display test: the next one,
            // and the menu after the last.
            Event::Touch((_, Touch::Tap(_))) => {
                if let Page::Display { pattern } = self.page {
                    if pattern + 1 >= PATTERNS {
                        self.show_menu();
                    } else {
                        self.show_display(pattern + 1);
                    }
                }
            }
            Event::Key(key) => self.key(key),
            Event::Hook(off_hook) => {
                let name = if off_hook { "hook-up" } else { "hook-down" };
                self.key_tested(name.to_string(), None);
            }
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Diagnostics,
    name: "diagnostics",
    id: "org.opendeskphone.diagnostics",
    display_name: "Diagnostics",
    description: "Hardware tests, subsystems, and the log",
    phone_icon: Some(Icon::AppDiagnostics),
}
