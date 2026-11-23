//! opendeskphone-ui: the phone's screen, keys, and lamps, on their own thread.
//!
//! The display the UI draws on is wrapped in a tap that keeps a copy of
//! what was last flushed, so the web UI's screen mirror works on the phone
//! as well as in the simulator.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use opendeskphone_config::Config;
use opendeskphone_hal::{Display, Frame, FrameInfo, Rect, Screen};
use opendeskphone_state::{HealthStatus, Subsystem};
use opendeskphone_ui::dialplan::DialPlan;
use opendeskphone_ui::{Ui, UiConfig, UiHandle};
use log::{error, info};

use super::SharedConfig;
use super::hal::SharedHal;
use super::system::ConfigHub;
use crate::mainrole::Context;

/// The picture on each screen, for whoever looks at it from another
/// thread (the web interface's mirror, `dphone screenshot`).
///
/// A copy of a screen is taken when somebody asks for the picture, not
/// when the picture changes: a wallpaper in motion changes it many times
/// a second, and a copy of the color panel's frame is half a megabyte
/// (4 ms on a D60).  The UI's thread takes the copy, since the frame
/// buffer is the UI's to read.  Until the UI runs, every picture is
/// copied as it is shown: the start-up screens are few.
#[derive(Default)]
pub struct ScreenTap {
    /// Counts the pictures shown.
    generation: AtomicU64,
    /// The copies: for each screen, the frame and the count it is of.
    frames: Mutex<Vec<(Screen, Frame, u64)>>,
    /// A copy was taken.
    taken: Condvar,
    /// Asks the UI's thread for new copies; set once the UI runs.
    ask: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl ScreenTap {
    /// How long a caller waits for the UI's thread to take a copy before
    /// it settles for the last one (the UI may be busy, or stuck: its last
    /// picture is then what there is to see).
    const WAIT: Duration = Duration::from_millis(500);

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The picture on `screen`.  May wait for the UI's thread, up to
    /// [`ScreenTap::WAIT`]: not for a thread that must not block.
    pub fn capture(&self, screen: Screen) -> Option<Frame> {
        let generation = self.generation();
        let stale = |frames: &mut Vec<(Screen, Frame, u64)>| frames.iter().find(|(s, ..)| *s == screen).is_some_and(|(.., of)| *of < generation);
        let mut frames = self.frames.lock().ok()?;
        if let Some(ask) = self.ask.get()
            && stale(&mut frames)
        {
            ask();
            frames = self.taken.wait_timeout_while(frames, Self::WAIT, stale).ok()?.0;
        }
        frames.iter().find(|(s, ..)| *s == screen).map(|(_, f, _)| f.clone())
    }

    /// Keeps `frame`, the picture on `screen` when `generation` pictures
    /// had been shown.
    fn keep(&self, screen: Screen, info: FrameInfo, pixels: &[u8], generation: u64) {
        let Ok(mut frames) = self.frames.lock() else { return };
        match frames.iter_mut().find(|(s, ..)| *s == screen) {
            Some((_, f, of)) if f.pixels.len() == pixels.len() => {
                f.pixels.copy_from_slice(pixels);
                (f.info, *of) = (info, generation);
            }
            Some((_, f, of)) => (*f, *of) = (Frame { info, pixels: pixels.to_vec() }, generation),
            None => frames.push((screen, Frame { info, pixels: pixels.to_vec() }, generation)),
        }
        self.taken.notify_all();
    }
}

struct TapDisplay {
    inner: Box<dyn Display>,
    tap: Arc<ScreenTap>,
}

impl Display for TapDisplay {
    fn screens(&self) -> Vec<Screen> {
        self.inner.screens()
    }
    fn info(&self, screen: Screen) -> Option<FrameInfo> {
        self.inner.info(screen)
    }
    fn frame(&mut self, screen: Screen) -> opendeskphone_hal::Result<&mut [u8]> {
        self.inner.frame(screen)
    }
    fn flush(&mut self, screen: Screen, damage: &[Rect]) -> opendeskphone_hal::Result<()> {
        let result = self.inner.flush(screen, damage);
        let generation = self.tap.generation.fetch_add(1, Ordering::Relaxed) + 1;
        // Before the UI runs, nobody can ask it for a copy.
        if self.tap.ask.get().is_none()
            && let (Some(info), Ok(pixels)) = (self.inner.info(screen), self.inner.frame(screen))
        {
            self.tap.keep(screen, info, pixels, generation);
        }
        result
    }
    /// On the UI's thread ([`UiHandle::capture_screens`]): the copy for
    /// the others is taken here.
    fn capture(&self, screen: Screen) -> opendeskphone_hal::Result<Frame> {
        let generation = self.tap.generation();
        let frame = self.inner.capture(screen)?;
        self.tap.keep(screen, frame.info, &frame.pixels, generation);
        Ok(frame)
    }
    fn set_power(&mut self, screen: Screen, on: bool) -> opendeskphone_hal::Result<()> {
        self.inner.set_power(screen, on)
    }
}

fn ui_config(ctx: &Context, cfg: Option<&Config>) -> UiConfig {
    // The built-in ringtones, and then the user's own in `/nvdata/ringtones`.
    let ringtones = super::ringtones::all(&ctx.paths.nvdata);
    let mut out = UiConfig { ringtones, ..UiConfig::for_model(ctx.store.model()) };
    out.ringtone = out.ringtones.first().cloned().unwrap_or_default();
    let Some(cfg) = cfg else { return out };
    out.time_24h = cfg.localization_setting("time_short_format").is_some_and(|f| f.contains('H') || f.contains('k'));
    out.utc_offset_secs = crate::tz::utc_offset(&cfg.time().time_zone, crate::sys::now_secs());
    if let Some(v) = cfg.setting_num::<u8>("brightness") {
        out.brightness = v.min(10);
    }
    out.dim_backlight = cfg.setting_bool("dim_backlight", out.dim_backlight);
    if let Some(v) = cfg.setting_num::<u8>("backlight_dim_level") {
        out.backlight_dim_level = v.min(10);
    }
    // Stock: 10 s when unset or over 600; 0 turns idle apps off.
    if let Some(v) = cfg.setting_num::<u32>("idle_cycle_seconds") {
        out.idle_cycle_secs = if v > 600 { 10 } else { v };
    }
    if let Some(v) = cfg.setting_num::<u32>("backlight_timeout") {
        out.backlight_timeout_secs = v;
    }
    if let Some(v) = cfg.setting_num::<u8>("contrast") {
        out.contrast = v;
    }
    let softkeys = cfg.idle_softkeys();
    warn_of_skipped_softkeys(&softkeys.skipped);
    out.idle_softkeys = idle_softkeys(&softkeys.keys);
    // The voicemail app reaches the server with its application account;
    // without one, the Messages key calls the voicemail number.
    out.voicemail_app_account = cfg.appconfig("voicemail").is_some_and(|app| app.child("account").is_some());
    out.park = cfg.setting("parking_lot_extension").is_some_and(|p| !p.is_empty());
    let network = cfg.network();
    out.network_dhcp = network.dhcp;
    out.vlan_discovery = network.vlan_discovery_mode;
    super::wallpaper::apply(cfg, &opendeskphone_config::Nvdata::new(&ctx.paths.nvdata), &mut out);
    let active = cfg.ringtones().active;
    if let Some(name) = out.ringtones.iter().find(|n| n.eq_ignore_ascii_case(&active)) {
        out.ringtone = name.clone();
    }
    out.ringtone_locked = cfg.setting_frozen("active_ringtone");
    // Stock: an account's `dial_plan`, else `default_dial_plan`; a plan
    // that does not compile is no plan.
    let default_plan = cfg.sip().default_dial_plan;
    out.default_dial_plan = dial_plan(&default_plan);
    out.dial_plans = cfg
        .accounts()
        .iter()
        .map(|a| {
            let plan = match a.dial_plan.as_deref() {
                None | Some("default") => out.default_dial_plan.clone(),
                Some(own) => dial_plan(own),
            };
            (a.account_id.clone(), plan)
        })
        .collect();
    out
}

fn dial_plan(plan: &str) -> Option<DialPlan> {
    // This runs every minute; one warning per plan is enough.
    static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    DialPlan::compile(plan)
        .map_err(|e| {
            let mut warned = WARNED.lock().unwrap();
            if !warned.iter().any(|w| w == plan) {
                warned.truncate(15);
                warned.push(plan.to_string());
                log::warn!("dial plan {plan:?} is not used: {e}");
            }
        })
        .ok()
}

fn warn_of_skipped_softkeys(skipped: &[String]) {
    // This also runs every minute, so each name gets one warning.
    static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut warned = WARNED.lock().unwrap();
    for item in skipped {
        if !warned.contains(item) {
            warned.truncate(15);
            warned.push(item.clone());
            log::warn!("opendeskphone_idle_softkeys: {item:?} is skipped: it names no softkey");
        }
    }
}

/// The configured idle softkeys as the UI's.
fn idle_softkeys(keys: &[opendeskphone_config::IdleSoftkey]) -> Vec<opendeskphone_ui::IdleSoftkey> {
    use opendeskphone_config::{IdleFunction, IdleSoftkey};
    keys.iter()
        .map(|key| match key {
            IdleSoftkey::Function(IdleFunction::CallLog) => opendeskphone_ui::IdleSoftkey::CallLog,
            IdleSoftkey::Function(IdleFunction::Contacts) => opendeskphone_ui::IdleSoftkey::Contacts,
            IdleSoftkey::Function(IdleFunction::Status) => opendeskphone_ui::IdleSoftkey::Status,
            IdleSoftkey::Function(IdleFunction::Menu) => opendeskphone_ui::IdleSoftkey::Menu,
            IdleSoftkey::Function(IdleFunction::Apps) => opendeskphone_ui::IdleSoftkey::Apps,
            IdleSoftkey::Function(IdleFunction::Dnd) => opendeskphone_ui::IdleSoftkey::Dnd,
            IdleSoftkey::Function(IdleFunction::Forward) => opendeskphone_ui::IdleSoftkey::Forward,
            IdleSoftkey::Function(IdleFunction::Redial) => opendeskphone_ui::IdleSoftkey::Redial,
            IdleSoftkey::Function(IdleFunction::Theme) => opendeskphone_ui::IdleSoftkey::Theme,
            IdleSoftkey::Function(IdleFunction::Voicemail) => opendeskphone_ui::IdleSoftkey::Voicemail,
            IdleSoftkey::App(name) => opendeskphone_ui::IdleSoftkey::App(name.clone()),
        })
        .collect()
}

pub struct UiParts {
    pub handle: UiHandle,
    pub tap: Arc<ScreenTap>,
}

pub async fn start(
    ctx: &Context,
    hal: Option<SharedHal>,
    config: Option<SharedConfig>,
    hub: &ConfigHub,
) -> Option<UiParts> {
    ctx.store.set_health(Subsystem::Ui, HealthStatus::Starting);
    // Declared before anything can return: a phone without its UI is not
    // healthy, whatever the reason, and must not confirm a trial boot.
    ctx.health.require("ui");
    let Some(hal) = hal else {
        error!("ui: no hardware layer; the UI is not started");
        ctx.store.set_health(Subsystem::Ui, HealthStatus::Failed("no hardware layer".into()));
        return None;
    };
    let tap = Arc::new(ScreenTap::default());
    let current = config.as_ref().map(|c| c.lock().unwrap().config());
    let cfg = ui_config(ctx, current.as_deref());
    let health = ctx.health.clone();
    let store = ctx.store.clone();
    // The UI is built on its own thread as well as run there, and draws
    // its first frame (the boot screen) there before it reports back: that
    // frame turns the panels on, which is a blocking device call (a mode
    // set that waits for the panel), and a display that does not answer
    // must not stall the runtime that SSH and the diagnostics run on.
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let thread_tap = tap.clone();
    let spawned = std::thread::Builder::new().name("ui".into()).spawn(move || {
        // The UI owns its devices and is not Sync; it lives on this thread.
        let ui_store = store.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut ui = {
                let Ok(mut hal) = hal.lock() else { return };
                let inner = std::mem::replace(&mut hal.display, Box::new(opendeskphone_hal::Null("display")));
                hal.display = Box::new(TapDisplay { inner, tap: thread_tap.clone() });
                Ui::from_hal(&mut hal, ui_store, cfg)
            };
            // The first frame: the panels come on showing it.
            ui.pump();
            // From here on a copy of a screen is taken when it is asked
            // for, by this thread.
            let handle = ui.handle();
            let _ = thread_tap.ask.set(Box::new(move || handle.capture_screens()));
            let _ = handle_tx.send(ui.handle());
            ui.run()
        }));
        health.fail("ui");
        let why = if result.is_err() { "the UI thread panicked" } else { "the UI thread ended" };
        error!("ui: {why}");
        store.set_health(Subsystem::Ui, HealthStatus::Failed(why.into()));
    });
    if let Err(e) = spawned {
        error!("ui: cannot start the UI thread: {e}");
        ctx.store.set_health(Subsystem::Ui, HealthStatus::Failed(e.to_string()));
        return None;
    }
    let handle = match tokio::time::timeout(Duration::from_secs(20), handle_rx).await {
        Ok(Ok(handle)) => handle,
        Ok(Err(_)) => {
            ctx.store.set_health(Subsystem::Ui, HealthStatus::Failed("the UI did not start".into()));
            return None;
        }
        Err(_) => {
            error!("ui: the display did not answer within 20 s; continuing without the UI");
            ctx.health.fail("ui");
            ctx.store.set_health(Subsystem::Ui, HealthStatus::Failed("the display does not answer".into()));
            return None;
        }
    };
    info!("ui: running");

    // Healthy once it has stayed up for a moment.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if !matches!(
                ctx.store.get::<opendeskphone_state::Health>().subsystems.iter().find(|s| s.subsystem == Subsystem::Ui).map(|s| &s.status),
                Some(HealthStatus::Failed(_))
            ) {
                ctx.health.satisfy("ui");
                ctx.store.set_health(Subsystem::Ui, HealthStatus::Ok);
            }
        });
    }

    // Follow the configuration, and re-evaluate the zone's offset now and
    // then (daylight saving changes, and the clock being set by SNTP).
    {
        let handle = handle.clone();
        let ctx2 = ctx.clone();
        hub.on_change(move |cfg| handle.set_config(ui_config(&ctx2, Some(cfg))));
    }
    if let Some(config) = config {
        let handle = handle.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut last = None;
            loop {
                let cfg = config.lock().unwrap().config();
                let next = ui_config(&ctx, Some(&cfg));
                if last.as_ref() != Some(&next) {
                    handle.set_config(next.clone());
                    last = Some(next);
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }
    Some(UiParts { handle, tap })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Draws a frame whose every byte is `value` on the main screen.
    fn show(display: &mut TapDisplay, value: u8) {
        display.frame(Screen::Main).unwrap().fill(value);
        display.flush(Screen::Main, &[]).unwrap();
    }

    /// Until the UI runs, the tap has a copy of every picture shown.  From
    /// then on no copy is taken when a picture is shown: whoever asks for
    /// the picture gets a copy the UI's thread takes then, and, if the UI
    /// does not answer, the last copy there is.
    #[test]
    fn the_tap_copies_a_screen_when_it_is_asked_for() {
        let tap = Arc::new(ScreenTap::default());
        let mut display = TapDisplay { inner: opendeskphone_hal::Hal::sim(opendeskphone_state::Model::D60).display, tap: tap.clone() };
        assert!(tap.capture(Screen::Main).is_none(), "nothing was shown");
        show(&mut display, 1);
        let first = tap.capture(Screen::Main).expect("the first picture");
        assert!(first.pixels.iter().all(|b| *b == 1));
        assert_eq!(tap.generation(), 1);

        // The UI runs: a request goes to its thread, which reads the
        // screen back through the display.
        let (asked, requests) = mpsc::channel();
        assert!(tap.ask.set(Box::new(move || asked.send(()).unwrap())).is_ok());
        show(&mut display, 2);
        show(&mut display, 3);
        assert_eq!(tap.generation(), 3);
        assert!(requests.try_recv().is_err(), "nobody has asked for the picture");
        let ui = std::thread::spawn(move || {
            requests.recv().unwrap();
            display.capture(Screen::Main).unwrap();
            // The next request is not answered.
            show(&mut display, 4);
            requests.recv().unwrap();
            display
        });
        let now = tap.capture(Screen::Main).expect("the picture as it is");
        assert!(now.pixels.iter().all(|b| *b == 3));
        // The copy is of the picture on the screen: no request while
        // nothing new is shown.  (The thread shows the next picture
        // before it waits for the second request.)
        let started = std::time::Instant::now();
        while tap.generation() < 4 {
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::yield_now();
        }
        let stale = tap.capture(Screen::Main).expect("the last copy");
        assert!(started.elapsed() >= ScreenTap::WAIT);
        assert!(stale.pixels.iter().all(|b| *b == 3), "the UI did not answer: the copy it took last");
        drop(ui.join().unwrap());
        // A screen this model does not have: nothing, at once.
        let started = std::time::Instant::now();
        assert!(tap.capture(Screen::Side).is_none());
        assert!(started.elapsed() < ScreenTap::WAIT);
    }

    #[test]
    fn the_clock_has_24_hours_when_the_locales_time_format_has() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context::standalone(
            crate::paths::Paths::new(dir.path().join("nvdata"), dir.path().join("run")),
            crate::model::Model::D60,
        );
        let cfg = Config::from_merged(
            opendeskphone_config::xml::Element::parse(concat!(
                "<config>",
                "<setting id=\"locale\" value=\"de_DE\"/>",
                "<locales><locale id=\"de_DE\"><setting id=\"time_short_format\" value=\"HH:mm\"/></locale></locales>",
                "</config>",
            ))
            .unwrap(),
            opendeskphone_config::Model::D60,
            None,
        );
        assert!(ui_config(&ctx, Some(&cfg)).time_24h);
    }

    fn config(ctx: &Context, xml: &str) -> Config {
        Config::from_merged(opendeskphone_config::xml::Element::parse(xml).unwrap(), ctx.model.into(), None)
    }

    #[test]
    fn the_d80s_default_softkeys_are_its_home_buttons() {
        use opendeskphone_ui::IdleSoftkey;
        let dir = tempfile::tempdir().unwrap();
        let d80 = Context::standalone(
            crate::paths::Paths::new(dir.path().join("nvdata"), dir.path().join("run")),
            crate::model::Model::D80,
        );
        assert_eq!(ui_config(&d80, None).idle_softkeys, IdleSoftkey::touch_defaults());
        assert_eq!(ui_config(&d80, Some(&config(&d80, "<config/>"))).idle_softkeys, IdleSoftkey::touch_defaults());
    }

    /// The UI is told whether the voicemail app can reach the server (it
    /// has its application account), and whether a call can be parked.
    #[test]
    fn the_ui_knows_of_the_voicemail_apps_account_and_the_parking_extension() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context::standalone(
            crate::paths::Paths::new(dir.path().join("nvdata"), dir.path().join("run")),
            crate::model::Model::D65,
        );
        let plain = ui_config(&ctx, Some(&config(&ctx, "<config><appconfig id=\"voicemail\"/></config>")));
        assert!(!plain.voicemail_app_account && !plain.park);
        let set = ui_config(
            &ctx,
            Some(&config(&ctx, concat!(
                "<config>",
                "<setting id=\"parking_lot_extension\" value=\"700\"/>",
                "<appconfig id=\"voicemail\"><account account_id=\"100\" username=\"u\" password=\"p\"/></appconfig>",
                "</config>",
            ))),
        );
        assert!(set.voicemail_app_account && set.park);
    }

    #[test]
    fn the_ui_knows_whether_the_ringtone_is_frozen() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context::standalone(
            crate::paths::Paths::new(dir.path().join("nvdata"), dir.path().join("run")),
            crate::model::Model::D65,
        );
        let fallback = config(&ctx, "<config><setting id=\"default_ringtone\" value=\"Solarium\" conflict=\"freeze\"/></config>");
        assert!(!ui_config(&ctx, Some(&fallback)).ringtone_locked);
        let frozen = config(&ctx, "<config><setting id=\"active_ringtone\" value=\"Digium\" conflict=\"freeze\"/></config>");
        assert!(ui_config(&ctx, Some(&frozen)).ringtone_locked);
    }
}
