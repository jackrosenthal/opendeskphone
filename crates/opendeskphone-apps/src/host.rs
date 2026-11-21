//! The host side of the app interface
//! (`crates/opendeskphone-app-sdk/wit/app.wit`): per-app context and the
//! implementation of every imported interface.

use crate::localstore::LocalStore;
use crate::manifest::{Capability, Manifest};
use crate::services::{
    APP_VISIBLE_SETTINGS, Completion, HttpRequest, PbxRequest, Services, StoreEntry, StoreOp,
    StoreReply, StoreScope, StoreSignal,
};
use crate::view::{AppWindow, Lamp};
use opendeskphone_state::{
    AccountId, AudioPath, BlfState, Call, CallId, CallState as StCallState, Command, Direction,
    Key, LampColor, PresenceStatus, Reply, Store,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        // The interface is defined once, in the SDK crate, which carries
        // it to crates.io.
        path: "../opendeskphone-app-sdk/wit",
        world: "app",
        additional_derives: [PartialEq],
    });
}

use bindings::opendeskphone::app as api;
use bindings::opendeskphone::app::types::*;

/// Widget handles with this bit belong to the idle window.
pub(crate) const IDLE_BIT: u32 = 1 << 31;
/// Free space always left on the filesystem of the apps and their stores
/// (stock: 2,048 KiB).
pub const FREE_RESERVE: u64 = 2048 * 1024;

/// Wakes the manager's loop from any thread.
#[derive(Default)]
pub(crate) struct Notifier {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Notifier {
    pub fn notify(&self) {
        *self.flag.lock().unwrap() = true;
        self.cv.notify_all();
    }
    /// Waits until notified or the timeout passes; clears the flag.
    pub fn wait(&self, timeout: Duration) {
        let g = self.flag.lock().unwrap();
        let (mut g, _) = self.cv.wait_timeout_while(g, timeout, |set| !*set).unwrap();
        *g = false;
    }
}

impl std::task::Wake for Notifier {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
}

/// A result that arrived from another thread.
pub(crate) enum Inbound {
    Pbx {
        app: String,
        generation: u64,
        id: u32,
        outcome: Result<PbxResult, PbxError>,
    },
    Http {
        app: String,
        generation: u64,
        id: u32,
        result: Result<HttpResponse, String>,
    },
    Store {
        app: String,
        generation: u64,
        id: u32,
        outcome: StorageOutcome,
    },
    /// The PBX's store came within reach, or a key changed there.
    StoreSignal(StoreSignal),
    /// The worker thread finished a compilation.
    Compiled(crate::prepare::Outcome),
}

#[derive(Default)]
pub(crate) struct Inbox {
    pub queue: Mutex<VecDeque<Inbound>>,
    pub notifier: Arc<Notifier>,
}

impl Inbox {
    pub fn push(&self, item: Inbound) {
        self.queue.lock().unwrap().push_back(item);
        self.notifier.notify();
    }
}

/// Something an app asked the manager to do, applied after the guest call
/// returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Background,
    Exit,
    Restart,
    ShowApp(String),
    Send {
        app: String,
        name: String,
        payload: String,
    },
    /// The app began or stopped watching its store on the PBX.
    StoreWatch,
}

pub(crate) struct Timer {
    pub id: u32,
    pub deadline_ms: u64,
    pub repeat_ms: Option<u32>,
}

/// The time limits of the guest call in progress (`start` or one
/// `handle`).  `limit` counts the time since the call began, less the
/// time host functions spent waiting for the phone's answer; `whole`
/// counts all of the time since the call began.
pub(crate) struct Budget {
    limit: Duration,
    whole: Duration,
    since: Instant,
    waited: Duration,
}

impl Budget {
    pub fn new(limit: Duration, whole: Duration) -> Budget {
        Budget {
            limit,
            whole,
            since: Instant::now(),
            waited: Duration::ZERO,
        }
    }

    /// What is left until the first of the two limits; zero when the app's
    /// code has run for all of `limit`, or the call has taken all of
    /// `whole`.
    pub fn left(&self) -> Duration {
        let elapsed = self.since.elapsed();
        let ran = elapsed.saturating_sub(self.waited);
        self.limit.saturating_sub(ran).min(self.whole.saturating_sub(elapsed))
    }

    /// What is left of `whole`.
    pub fn whole_left(&self) -> Duration {
        self.whole.saturating_sub(self.since.elapsed())
    }

    /// An error once the call has taken all of `whole`: the app is stopped.
    pub fn in_time(&self) -> wasmtime::Result<()> {
        if self.whole_left().is_zero() {
            wasmtime::bail!(
                "the call took {} s in all, waits for the phone's answer included",
                self.whole.as_secs_f32()
            );
        }
        Ok(())
    }
}

pub(crate) struct AppCtx {
    pub name: String,
    pub manifest: Manifest,
    /// Distinguishes this instance from an earlier one of the same app, so
    /// late replies for a dead instance are dropped.
    pub generation: u64,
    /// The app's keys on the phone (`/nvdata/appdata/<name>`).
    pub local: LocalStore,
    pub window: Box<dyn AppWindow>,
    pub idle: Option<Box<dyn AppWindow>>,
    pub idle_size: Size,
    pub store: Store,
    pub services: Services,
    pub inbox: Arc<Inbox>,
    pub actions: Vec<Action>,
    pub timers: Vec<Timer>,
    pub next_timer: u32,
    pub next_request: u32,
    pub pending: HashSet<u32>,
    pub observed: HashSet<String>,
    pub observe_calls: bool,
    /// The app wants `event.storage-changed`.
    pub store_watch: bool,
    /// The presence targets the app observes.
    pub presence_observed: HashSet<String>,
    /// The app called `hotdesk.login` or `logout`, and has not been told
    /// how it ended.
    pub hotdesk_pending: bool,
    pub exit_after_background: bool,
    pub foreground: bool,
    /// The pictures the app defined (`ui.define-image`), by name.
    pub pictures: HashMap<String, Arc<[u8]>>,
    /// The pictures already handed to a window (idle or not), by name.
    pub images: HashSet<(bool, String)>,
    /// A sound file of this app is playing.
    pub playing: bool,
    /// Until when (monotonic milliseconds) the alarm tone preview this app
    /// started sounds; 0 when the app has none.
    pub preview_until_ms: u64,
    pub limits: wasmtime::StoreLimits,
    pub budget: Budget,
}

/// How long an alarm tone preview sounds (the audio side ends it then).
const PREVIEW_MS: u64 = 4000;

/// The most timers an app may have at a time.
const TIMERS_MAX: usize = 32;
/// The most presence targets an app may observe, and the most push names.
const PRESENCE_MAX: usize = 256;
const OBSERVED_MAX: usize = 64;

/// The largest picture an app may define, and the most pictures.
const IMAGE_MAX: usize = 512 * 1024;
const IMAGES_MAX: usize = 32;

fn bad_name(name: &str) -> bool {
    name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0'])
}

/// Bytes a write may still use, honoring the reserve.
// The statvfs field widths differ between 32-bit and 64-bit targets.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn usable_space(dir: &Path) -> u64 {
    match rustix::fs::statvfs(dir) {
        Ok(s) => (s.f_bavail as u64)
            .saturating_mul(s.f_frsize as u64)
            .saturating_sub(FREE_RESERVE),
        Err(_) => 0,
    }
}

pub(crate) fn call_info(c: &Call) -> CallInfo {
    let state = match c.state {
        StCallState::Dialing => CallState::Dialing,
        StCallState::Proceeding => CallState::Proceeding,
        StCallState::Ringback { .. } => CallState::Ringback,
        StCallState::Ringing => CallState::Ringing,
        StCallState::Active => CallState::Active,
        StCallState::Held { .. } => CallState::Held,
        StCallState::Transferring => CallState::Transferring,
        StCallState::Ended(_) => CallState::Ended,
    };
    let duration = c
        .connected_mono_ms
        .map(|t| (opendeskphone_state::clock::mono_ms().saturating_sub(t) / 1000) as u32)
        .unwrap_or(0);
    CallInfo {
        id: c.id.0,
        account: c.account.0,
        incoming: c.direction == Direction::Incoming,
        state,
        remote_name: c.remote.name.clone(),
        remote_number: c.remote.number.clone(),
        duration,
    }
}

fn presence_status(s: PresenceStatus) -> api::types::PresenceStatus {
    use api::types::PresenceStatus as P;
    match s {
        PresenceStatus::Available => P::Available,
        PresenceStatus::Chat => P::Chat,
        PresenceStatus::Away => P::Away,
        PresenceStatus::Dnd => P::Dnd,
        PresenceStatus::ExtendedAway => P::Xa,
        PresenceStatus::Unavailable => P::Unavailable,
        PresenceStatus::Unknown => P::Unknown,
    }
}

pub(crate) fn presence_of(e: &opendeskphone_state::BlfEntry, target: &str) -> Presence {
    let line_state = match e.state {
        BlfState::Unknown => LineState::Unknown,
        BlfState::Idle => LineState::Idle,
        BlfState::Ringing => LineState::Ringing,
        BlfState::InUse => LineState::Busy,
        BlfState::OnHold => LineState::Hold,
        BlfState::OnHoldRinging => LineState::HoldRinging,
        BlfState::Unavailable => LineState::Unavailable,
    };
    Presence {
        target: target.to_string(),
        status: presence_status(e.status),
        text: e.status_text.clone(),
        line_state,
    }
}

/// A key as the app interface has it.
pub(crate) fn wit_key(k: Key) -> api::types::Key {
    use api::types::Key as K;
    match k {
        Key::Digit(d) => K::Digit(d),
        Key::Star => K::Star,
        Key::Pound => K::Pound,
        Key::Up => K::Up,
        Key::Down => K::Down,
        Key::Left => K::Left,
        Key::Right => K::Right,
        Key::Select => K::Select,
        Key::Cancel => K::Cancel,
        Key::VolumeUp => K::VolumeUp,
        Key::VolumeDown => K::VolumeDown,
        Key::Soft(n) => K::Softkey(n),
        Key::Line(n) => K::Line(n),
        Key::Blf(n) => K::Blf(n),
        Key::Mute => K::Mute,
        Key::Speaker => K::Speaker,
        Key::Headset => K::Headset,
        Key::Hold => K::Hold,
        Key::Transfer => K::Transfer,
        Key::Redial => K::Redial,
        Key::Messages => K::Messages,
        Key::Conference => K::Conference,
        Key::Info => K::Info,
        Key::BlfPageLeft => K::BlfPageLeft,
        Key::BlfPageRight => K::BlfPageRight,
        Key::Status => K::Status,
        Key::Contacts => K::Contacts,
        Key::Apps => K::Apps,
        Key::Menu => K::Menu,
        Key::Expansion { module, key } => K::Expansion(ExpansionKey { module, key }),
        Key::Unknown(code) => K::Other(code),
    }
}

/// The phone's key for a key of the app interface; `None` for a number
/// out of range (a digit over 9, softkey 5, line key 0).
pub(crate) fn phone_key(k: &api::types::Key) -> Option<Key> {
    use api::types::Key as K;
    let numbered = |n: u8, max: u8, f: fn(u8) -> Key| (1..=max).contains(&n).then(|| f(n));
    Some(match *k {
        K::Digit(d) if d <= 9 => Key::Digit(d),
        K::Digit(_) => return None,
        K::Star => Key::Star,
        K::Pound => Key::Pound,
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Select => Key::Select,
        K::Cancel => Key::Cancel,
        K::VolumeUp => Key::VolumeUp,
        K::VolumeDown => Key::VolumeDown,
        K::Softkey(n) => return numbered(n, 4, Key::Soft),
        K::Line(n) => return numbered(n, 6, Key::Line),
        K::Blf(n) => return numbered(n, 10, Key::Blf),
        K::Mute => Key::Mute,
        K::Speaker => Key::Speaker,
        K::Headset => Key::Headset,
        K::Hold => Key::Hold,
        K::Transfer => Key::Transfer,
        K::Redial => Key::Redial,
        K::Messages => Key::Messages,
        K::Conference => Key::Conference,
        K::Info => Key::Info,
        K::BlfPageLeft => Key::BlfPageLeft,
        K::BlfPageRight => Key::BlfPageRight,
        K::Status => Key::Status,
        K::Contacts => Key::Contacts,
        K::Apps => Key::Apps,
        K::Menu => Key::Menu,
        K::Expansion(e) => Key::Expansion { module: e.module, key: e.key },
        K::Other(code) => Key::Unknown(code),
    })
}

/// A lamp as the app interface has it (line and rapid-dial lamps from 1).
fn wit_lamp(l: Lamp) -> api::lamps::Lamp {
    use api::lamps::Lamp as L;
    match l {
        Lamp::Line(i) => L::Line(i.saturating_add(1)),
        Lamp::Blf(i) => L::Blf(i.saturating_add(1)),
        Lamp::Message => L::Message,
        Lamp::Mute => L::Mute,
        Lamp::Headset => L::Headset,
        Lamp::Speaker => L::Speaker,
    }
}

/// The method of a push as the SIP side names it.
pub(crate) fn push_method(method: &str) -> PushMethod {
    match method {
        "MESSAGE" => PushMethod::Message,
        "INVITE" => PushMethod::Invite,
        _ => PushMethod::Info,
    }
}

/// True when a BLF entry is the one an app means by `target` (an
/// extension, a contact id, or a URI).
pub(crate) fn entry_matches(e: &opendeskphone_state::BlfEntry, target: &str) -> bool {
    e.number == target || e.id == target || e.uri == target
}

/// The reply document of a PBX request, read as `read` reads the
/// method's result.
pub(crate) fn pbx_outcome(
    reply: Result<Vec<u8>, String>,
    read: crate::pbxapi::Reader,
) -> Result<PbxResult, PbxError> {
    let bytes = reply.map_err(PbxError::Transport)?;
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Err(PbxError::Transport("the server's reply was not JSON".into()));
    };
    let Some(r) = v.get("response") else {
        return Err(PbxError::Transport("the server's reply had no response".into()));
    };
    if let Some(e) = r.get("errors") {
        return Err(PbxError::Server(crate::pbxapi::server_error(e)));
    }
    let Some(result) = r.get("result") else {
        return Err(PbxError::Transport("the server's reply had no result".into()));
    };
    read(result).ok_or_else(|| PbxError::Transport("the server's reply was not the method's".into()))
}

/// Days-from-civil inverse: (year, month, day) for days since 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub(crate) fn format_time(unix: u64, offset: i32, h24: bool, style: api::l10n::TimeStyle) -> String {
    use api::l10n::TimeStyle;
    let t = unix as i64 + offset as i64;
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    let (hh, mm) = (secs / 3600, secs % 3600 / 60);
    let time = if h24 {
        format!("{hh:02}:{mm:02}")
    } else {
        let h12 = match hh % 12 {
            0 => 12,
            h => h,
        };
        format!("{h12}:{mm:02} {}", if hh < 12 { "AM" } else { "PM" })
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = format!("{} {d}, {y}", MONTHS[(m - 1) as usize]);
    match style {
        TimeStyle::Time => time,
        TimeStyle::Date => date,
        TimeStyle::DateAndTime => format!("{date} {time}"),
    }
}

/// What an app gets for a command the phone did not carry out, under the
/// kind the interface has for it.
pub(crate) fn command_error(e: opendeskphone_state::CommandError) -> Error {
    use opendeskphone_state::CommandError;
    match e {
        CommandError::Invalid(why) => Error::Invalid(why),
        CommandError::Denied(why) => Error::Failed(format!("the phone refused: {why}")),
        CommandError::Failed(why) => Error::Failed(why),
        CommandError::Timeout => Error::Failed(
            "the phone did not answer in time; what was asked may still happen".into(),
        ),
        e @ (CommandError::NoHandler(_) | CommandError::Dropped) => Error::Failed(e.to_string()),
    }
}

impl AppCtx {
    fn win(&mut self, w: u32) -> Option<(&mut (dyn AppWindow + 'static), Option<u32>)> {
        if w & IDLE_BIT != 0 {
            let id = w & !IDLE_BIT;
            self.idle
                .as_deref_mut()
                .map(|i| (i, (id != 0).then_some(id)))
        } else {
            Some((self.window.as_mut(), (w != 0).then_some(w)))
        }
    }

    /// Runs `f` on the window owning `w` when `w` names a real widget.
    fn on_widget(&mut self, w: u32, f: impl FnOnce(&mut dyn AppWindow, u32)) {
        if let Some((win, Some(id))) = self.win(w) {
            f(win, id);
        }
    }

    /// Hands the window the picture the app defined as `name`, the first
    /// time the app names it there.
    fn load_image(&mut self, w: u32, name: &str) {
        let idle = w & IDLE_BIT != 0;
        let Some(png) = self.pictures.get(name).cloned() else {
            return;
        };
        if !self.images.insert((idle, name.to_string())) {
            return;
        }
        let app = self.name.clone();
        if let Some((win, _)) = self.win(w)
            && !win.define_image(name, &png)
        {
            log::warn!("app {app}: {name} is not a usable picture");
        }
    }

    /// Sends a command and waits up to `timeout` for the phone's answer,
    /// and no longer than the guest call has left of the budget's `whole`.
    /// The wait does not count toward the budget's `limit`.
    fn wait_for(&mut self, c: Command, timeout: Duration) -> Result<Reply, Error> {
        let timeout = timeout.min(self.budget.whole_left());
        if timeout.is_zero() {
            // The app is stopped before the call ends; nothing is sent.
            return Err(Error::Failed("the app ran out of time".into()));
        }
        let since = Instant::now();
        let answer = self.store.send(c).wait_timeout(timeout);
        self.budget.waited += since.elapsed();
        answer.map_err(command_error)
    }

    fn command(&mut self, c: Command) -> Result<Reply, Error> {
        self.wait_for(c, Duration::from_millis(500))
    }

    /// The apps the phone offers, as the manager last published them.
    fn peers(&self) -> Arc<opendeskphone_state::Apps> {
        self.store.get::<opendeskphone_state::Apps>()
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.manifest.has(cap)
    }
}

impl api::types::Host for AppCtx {}

impl api::runtime::Host for AppCtx {
    fn background(&mut self) {
        self.actions.push(Action::Background);
    }
    fn exit(&mut self) {
        self.actions.push(Action::Exit);
    }
    fn restart(&mut self) {
        self.actions.push(Action::Restart);
    }
    fn set_exit_after_background(&mut self, exit: bool) {
        self.exit_after_background = exit;
    }
    fn show_app(&mut self, name: String) -> Result<(), String> {
        if crate::manifest::validate_name(&name).is_err() {
            return Err("bad app name".into());
        }
        let apps = self.peers();
        if apps.disabled.iter().any(|a| a.name == name) {
            return Err(format!("the app {name:?} is switched off"));
        }
        if !apps.installed.iter().any(|a| a.name == name) {
            return Err(format!("no app named {name:?}"));
        }
        self.actions.push(Action::ShowApp(name));
        Ok(())
    }
    fn reset_idle_timer(&mut self) {
        // As on stock: only the app on screen keeps the phone awake.
        if self.foreground {
            let _ = self.store.post(Command::ResetIdleTimer);
        }
    }
    fn log(&mut self, level: u8, message: String) {
        let level = match level {
            0 => log::Level::Error,
            1 => log::Level::Warn,
            2 => log::Level::Info,
            _ => log::Level::Debug,
        };
        let mut msg = message;
        if msg.len() > 512 {
            let mut end = 512;
            while !msg.is_char_boundary(end) {
                end -= 1;
            }
            msg.truncate(end);
        }
        log::log!(target: "app", level, "{}: {}", self.name, msg);
    }
    fn in_foreground(&mut self) -> bool {
        self.foreground
    }
    fn random(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        if getrandom::fill(&mut bytes).is_err() {
            // No kernel source (not on the phone): the clocks, stirred.
            let t = opendeskphone_state::clock::mono_ms() ^ opendeskphone_state::clock::unix_secs().rotate_left(32);
            return t.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29) ^ t;
        }
        u64::from_le_bytes(bytes)
    }
}

impl api::ui::Host for AppCtx {
    fn get_metrics(&mut self) -> Metrics {
        self.window.metrics()
    }
    fn idle_size(&mut self) -> Size {
        self.idle_size
    }
    fn idle_root(&mut self) -> Option<u32> {
        self.idle.is_some().then_some(IDLE_BIT)
    }
    fn create(&mut self, kind: WidgetKind, parent: u32, bounds: Rect) -> u32 {
        let idle = parent & IDLE_BIT;
        match self.win(parent) {
            Some((win, p)) => win.create(kind, p, bounds) | idle,
            None => 0,
        }
    }
    fn destroy(&mut self, w: u32) {
        self.on_widget(w, |win, id| win.destroy(id));
    }
    fn clear(&mut self, w: u32) {
        if let Some((win, id)) = self.win(w) {
            win.clear(id);
        }
    }
    fn set_bounds(&mut self, w: u32, bounds: Rect) {
        self.on_widget(w, |win, id| win.set_bounds(id, bounds));
    }
    fn set_visible(&mut self, w: u32, visible: bool) {
        self.on_widget(w, |win, id| win.set_visible(id, visible));
    }
    fn set_focus(&mut self, w: u32) {
        if let Some((win, id)) = self.win(w) {
            win.set_focus(id);
        }
    }
    fn set_text(&mut self, w: u32, text: String) {
        self.on_widget(w, |win, id| win.set_text(id, &text));
    }
    fn set_align(&mut self, w: u32, align: Align) {
        self.on_widget(w, |win, id| win.set_align(id, align));
    }
    fn set_font(&mut self, w: u32, font: Font) {
        self.on_widget(w, |win, id| win.set_font(id, font));
    }
    fn set_wrap(&mut self, w: u32, wrap: bool) {
        self.on_widget(w, |win, id| win.set_wrap(id, wrap));
    }
    fn set_highlight(&mut self, w: u32, on: bool) {
        self.on_widget(w, |win, id| win.set_highlight(id, on));
    }
    fn set_items(&mut self, w: u32, items: Vec<ListItem>) {
        for item in &items {
            if let Some(Image::Picture(name)) = &item.icon {
                self.load_image(w, name);
            }
        }
        self.on_widget(w, |win, id| win.set_items(id, &items));
    }
    fn set_selected(&mut self, w: u32, index: u32) {
        self.on_widget(w, |win, id| win.set_selected(id, index));
    }
    fn selected(&mut self, w: u32) -> Option<u32> {
        match self.win(w) {
            Some((win, Some(id))) => win.selected(id),
            _ => None,
        }
    }
    fn set_value(&mut self, w: u32, value: String) {
        self.on_widget(w, |win, id| win.set_value(id, &value));
    }
    fn value(&mut self, w: u32) -> String {
        match self.win(w) {
            Some((win, Some(id))) => win.value(id),
            _ => String::new(),
        }
    }
    fn set_input_mode(&mut self, w: u32, mode: InputMode) {
        self.on_widget(w, |win, id| win.set_input_mode(id, mode));
    }
    fn set_max_length(&mut self, w: u32, max: u32) {
        self.on_widget(w, |win, id| win.set_max_length(id, max));
    }
    fn define_image(&mut self, name: String, png: Vec<u8>) -> Result<(), Error> {
        if bad_name(&name) {
            return Err(Error::Invalid("a picture needs a plain name".into()));
        }
        if png.len() > IMAGE_MAX {
            return Err(Error::Invalid("a picture may be 512 KiB".into()));
        }
        if !self.pictures.contains_key(&name) && self.pictures.len() >= IMAGES_MAX {
            return Err(Error::Invalid("an app may define 32 pictures".into()));
        }
        // The app's window decodes the picture now, so that the app
        // learns here of content the phone cannot show.  A refused
        // picture leaves an earlier one of the name as it was.
        if !self.window.define_image(&name, &png) {
            return Err(Error::Invalid(
                "not a PNG picture the phone can show (at most 1,048,576 pixels)".into(),
            ));
        }
        self.pictures.insert(name.clone(), png.into());
        self.images.insert((false, name.clone()));
        // An idle window that has the old picture of this name gets the
        // new one now; otherwise when the app first uses the name there.
        if self.images.remove(&(true, name.clone())) {
            self.load_image(IDLE_BIT, &name);
        }
        Ok(())
    }
    fn set_image(&mut self, w: u32, image: Image) {
        if let Image::Picture(name) = &image {
            if bad_name(name) {
                return;
            }
            self.load_image(w, name);
        }
        self.on_widget(w, |win, id| win.set_image(id, &image));
    }
    fn claim_keys(&mut self, w: u32, keys: Vec<api::types::Key>) {
        let keys: Vec<Key> = keys.iter().filter_map(phone_key).collect();
        if let Some((win, id)) = self.win(w) {
            win.claim_keys(id, &keys);
        }
    }
    fn set_full_screen(&mut self, on: bool) {
        self.window.set_full_screen(on);
    }
    fn set_softkeys(&mut self, w: u32, keys: Vec<Softkey>) {
        if let Some((win, id)) = self.win(w) {
            win.set_softkeys(id, &keys);
        }
    }
}

impl api::timers::Host for AppCtx {
    fn set(&mut self, delay_ms: u32, repeat: bool) -> Result<u32, Error> {
        // A handful of timers is plenty; refuse to grow without bound.
        if self.timers.len() >= TIMERS_MAX {
            return Err(Error::Failed(format!("an app may have {TIMERS_MAX} timers")));
        }
        self.next_timer = self.next_timer.wrapping_add(1).max(1);
        let id = self.next_timer;
        // Repeating timers faster than 50 ms would starve the phone.
        let delay = if repeat { delay_ms.max(50) } else { delay_ms };
        self.timers.push(Timer {
            id,
            deadline_ms: opendeskphone_state::clock::mono_ms() + delay as u64,
            repeat_ms: repeat.then_some(delay),
        });
        Ok(id)
    }
    fn cancel(&mut self, id: u32) {
        self.timers.retain(|t| t.id != id);
    }
    fn now_ms(&mut self) -> u64 {
        opendeskphone_state::clock::mono_ms()
    }
    fn wall_time(&mut self) -> u64 {
        self.services.config.wall_time()
    }
}

impl api::config::Host for AppCtx {
    fn app_config(&mut self) -> String {
        self.services.config.app_config_xml(&self.name)
    }
    fn app_setting(&mut self, name: String) -> Option<String> {
        // Credentials stay with the host: it adds them to PBX requests.
        if matches!(name.as_str(), "password" | "secret") {
            return None;
        }
        if name == "account_id"
            && let Some(a) = self.services.config.app_account(&self.name)
        {
            return Some(a.account_id);
        }
        self.services.config.app_setting(&self.name, &name)
    }
    fn setting(&mut self, id: api::config::PhoneSetting) -> Option<String> {
        use api::config::PhoneSetting as S;
        // The ids of `APP_VISIBLE_SETTINGS`.
        let id = match id {
            S::Locale => "locale",
            S::TimeZone => "timezone",
            S::TimeFormat => "time_format",
            S::DateFormat => "date_format",
            S::ContactsMaxSubscriptions => "contacts_max_subscriptions",
            S::BlfContactGroup => "blf_contact_group",
            S::DefaultRingtone => "default_ringtone",
            S::NameFormat => "name_format",
            S::Brightness => "brightness",
        };
        debug_assert!(APP_VISIBLE_SETTINGS.contains(&id));
        self.services.config.setting(id)
    }
    fn accounts(&mut self) -> Vec<AccountInfo> {
        self.store
            .get::<opendeskphone_state::Accounts>()
            .accounts
            .iter()
            .map(|a| AccountInfo {
                slot: a.id.0,
                id: a.config_id.clone(),
                label: a.label.clone(),
                user: a.user.clone(),
                server: a.server.clone(),
                registered: a.registration.is_registered(),
            })
            .collect()
    }
    fn model(&mut self) -> PhoneModel {
        use api::types::PhoneModel as M;
        match self.store.model() {
            opendeskphone_state::Model::D40 => M::D40,
            opendeskphone_state::Model::D45 => M::D45,
            opendeskphone_state::Model::D50 => M::D50,
            opendeskphone_state::Model::D60 => M::D60,
            opendeskphone_state::Model::D62 => M::D62,
            opendeskphone_state::Model::D65 => M::D65,
            opendeskphone_state::Model::D70 => M::D70,
            // TODO(D80 app API): the app interface (`wit/app.wit`) has no
            // `d80` phone model yet; an app is told the model whose skin
            // it is given until then (see `uishell.rs`).
            opendeskphone_state::Model::D80 => M::D65,
        }
    }
    fn firmware_version(&mut self) -> String {
        self.store
            .get::<opendeskphone_state::System>()
            .version
            .underscored()
    }
    fn mac_address(&mut self) -> String {
        self.services.config.mac_address()
    }
    fn contact_lists(&mut self) -> Vec<(String, String)> {
        self.services.config.contact_lists()
    }
}

impl AppCtx {
    fn next_request(&mut self) -> u32 {
        self.next_request = self.next_request.wrapping_add(1).max(1);
        self.pending.insert(self.next_request);
        self.next_request
    }
}

impl AppCtx {
    /// Sends a request to the PBX's application server, with the app
    /// account's `account_id` (and, for `voicemail.forward`, as
    /// `account_from`); the result comes to the app as `event.pbx`.
    fn pbx_call(&mut self, call: crate::pbxapi::Call) -> u32 {
        let id = self.next_request();
        let inbox = self.inbox.clone();
        let (app, generation) = (self.name.clone(), self.generation);
        let read = call.read;
        let fail = |inbox: &Arc<crate::host::Inbox>, why: &str| {
            inbox.push(Inbound::Pbx {
                app: app.clone(),
                generation,
                id,
                outcome: Err(PbxError::Transport(why.to_string())),
            });
        };
        let Some(account) = self.services.config.app_account(&self.name) else {
            fail(&inbox, "this app has no application account configured");
            return id;
        };
        let mut params = call.parameters;
        if !account.account_id.is_empty() {
            let key = if call.method == "switchvox.users.voicemail.forward" {
                "account_from"
            } else {
                "account_id"
            };
            params.insert(key.into(), account.account_id.clone().into());
        }
        let body = serde_json::json!({"request": {"method": call.method, "parameters": params}})
            .to_string();
        if account.appserver.starts_with("dasp://") {
            let done = {
                let app = app.clone();
                Completion::new(move |reply| {
                    inbox.push(Inbound::Pbx {
                        app,
                        generation,
                        id,
                        outcome: pbx_outcome(reply, read),
                    })
                })
            };
            self.services.pbx.request(
                PbxRequest {
                    url: account.appserver,
                    username: account.username,
                    password: account.password,
                    body,
                    app: self.name.clone(),
                },
                done,
            );
        } else if account.appserver.starts_with("http://")
            || account.appserver.starts_with("https://")
        {
            let done = {
                let app = app.clone();
                Completion::new(move |reply: Result<crate::HttpReply, String>| {
                    let outcome = match reply {
                        Ok(r) if (200..300).contains(&r.status) => pbx_outcome(Ok(r.body), read),
                        Ok(r) => Err(PbxError::Transport(format!("HTTP status {}", r.status))),
                        Err(e) => Err(PbxError::Transport(e)),
                    };
                    inbox.push(Inbound::Pbx {
                        app,
                        generation,
                        id,
                        outcome,
                    })
                })
            };
            self.services.http.request(
                HttpRequest {
                    method: "POST".into(),
                    url: account.appserver,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: body.into_bytes(),
                    credentials: Some((account.username, account.password)),
                },
                done,
            );
        } else {
            fail(&inbox, "the application server address is not usable");
        }
        id
    }

    /// A request about a call of the phone: the call's account (its
    /// `id`) and SIP Call-ID; an error for a call the phone does not have.
    fn recording(&mut self, start: bool, call: u32) -> u32 {
        let calls = self.store.get::<opendeskphone_state::Calls>();
        let accounts = self.store.get::<opendeskphone_state::Accounts>();
        let found = calls.calls.iter().find(|c| c.id.0 == call).map(|c| {
            let line = accounts
                .accounts
                .iter()
                .find(|a| a.id == c.account)
                .map(|a| a.config_id.clone())
                .unwrap_or_default();
            (line, c.sip_call_id.clone())
        });
        match found {
            Some((line, call_id)) => self.pbx_call(crate::pbxapi::recording(start, line, call_id)),
            None => {
                let id = self.next_request();
                self.inbox.push(Inbound::Pbx {
                    app: self.name.clone(),
                    generation: self.generation,
                    id,
                    outcome: Err(PbxError::Transport("the phone has no such call".into())),
                });
                id
            }
        }
    }
}

impl api::pbx::Host for AppCtx {
    fn presence_info(&mut self) -> u32 {
        self.pbx_call(crate::pbxapi::presence_info())
    }
    fn presence_options(&mut self) -> u32 {
        self.pbx_call(crate::pbxapi::presence_options())
    }
    fn set_presence(&mut self, option_id: u32, message: String) -> u32 {
        self.pbx_call(crate::pbxapi::set_presence(option_id, message))
    }
    fn parking_lots(&mut self) -> u32 {
        self.pbx_call(crate::pbxapi::parking_lots())
    }
    fn start_recording(&mut self, call: u32) -> u32 {
        self.recording(true, call)
    }
    fn stop_recording(&mut self, call: u32) -> u32 {
        self.recording(false, call)
    }
    fn queues(&mut self) -> u32 {
        self.pbx_call(crate::pbxapi::queues())
    }
    fn queue_status(&mut self, queue: String) -> u32 {
        self.pbx_call(crate::pbxapi::queue_status(queue))
    }
    fn queue_login(&mut self, queue: Option<String>) -> u32 {
        self.pbx_call(crate::pbxapi::queue_login(queue))
    }
    fn queue_logout(&mut self, queue: Option<String>) -> u32 {
        self.pbx_call(crate::pbxapi::queue_logout(queue))
    }
    fn queue_pause(&mut self, queue: Option<String>, reason: String) -> u32 {
        self.pbx_call(crate::pbxapi::queue_pause(queue, reason))
    }
    fn queue_resume(&mut self, queue: Option<String>) -> u32 {
        self.pbx_call(crate::pbxapi::queue_resume(queue))
    }
    fn add_contact(&mut self, phonebook: String, extension: String, name: String) -> u32 {
        self.pbx_call(crate::pbxapi::add_contact(phonebook, extension, name))
    }
    fn voicemail_messages(
        &mut self,
        folder: String,
        sort: VoicemailSort,
        descending: bool,
        per_page: u32,
        page: u32,
    ) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_messages(folder, sort, descending, per_page, page))
    }
    fn voicemail_folders(&mut self) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_folders())
    }
    fn voicemail_move(&mut self, folder: String, target: String, messages: Vec<String>) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_move(folder, target, messages))
    }
    fn voicemail_remove(&mut self, folder: String, messages: Vec<String>) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_remove(folder, messages))
    }
    fn voicemail_forward(&mut self, folder: String, account: String, messages: Vec<String>) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_forward(account, folder, messages))
    }
    fn voicemail_mark_read(&mut self, folder: String, messages: Vec<String>) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_mark(true, folder, messages))
    }
    fn voicemail_mark_unread(&mut self, folder: String, messages: Vec<String>) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_mark(false, folder, messages))
    }
    fn voicemail_play(&mut self, folder: String, message: String) -> u32 {
        self.pbx_call(crate::pbxapi::voicemail_play(folder, message))
    }
    fn cancel(&mut self, id: u32) {
        self.pending.remove(&id);
    }
}

/// The PBX's scope of a location on the PBX.
fn store_scope(location: StorageLocation) -> Option<StoreScope> {
    match location {
        StorageLocation::Local => None,
        StorageLocation::PbxPrivate => Some(StoreScope::User),
        StorageLocation::PbxShared => Some(StoreScope::Shared),
    }
}

pub(crate) fn location(scope: StoreScope) -> StorageLocation {
    match scope {
        StoreScope::Shared => StorageLocation::PbxShared,
        StoreScope::User => StorageLocation::PbxPrivate,
    }
}

fn storage_entry(e: StoreEntry) -> StorageEntry {
    StorageEntry {
        key: e.key,
        value: e.value,
        version: e.version,
        modified: e.modified,
        author_id: e.author_id,
        author_name: e.author_name,
    }
}

/// What the app gets for a reply of a store.
pub(crate) fn storage_outcome(reply: StoreReply) -> StorageOutcome {
    match reply {
        StoreReply::Entry(e) => StorageOutcome::Entry(e.map(storage_entry)),
        StoreReply::Entries(list, more) => {
            StorageOutcome::Entries((list.into_iter().map(storage_entry).collect(), more))
        }
        StoreReply::Stored(e) => StorageOutcome::Stored(storage_entry(e)),
        StoreReply::Removed => StorageOutcome::Removed,
        StoreReply::Conflict(e) => StorageOutcome::Conflict(e.map(storage_entry)),
        StoreReply::Unreachable(why) => StorageOutcome::Unreachable(why),
        StoreReply::Refused(why) => StorageOutcome::Refused(why),
    }
}

impl AppCtx {
    /// Sends a request to the PBX's store; the reply comes to the app as
    /// `event.storage`.  The app's name goes with it from here, so an app
    /// reaches no keys but its own.
    fn store_request(&mut self, op: impl FnOnce(String) -> StoreOp) -> StorageReply {
        let id = self.next_request();
        let inbox = self.inbox.clone();
        let (app, generation) = (self.name.clone(), self.generation);
        let done = Completion::new(move |reply| {
            inbox.push(Inbound::Store {
                app,
                generation,
                id,
                outcome: storage_outcome(reply),
            })
        });
        self.services.store.request(op(self.name.clone()), done);
        StorageReply::Pending(id)
    }
}

/// `local` is answered at once; the PBX locations, by `event.storage`.
impl api::storage::Host for AppCtx {
    fn get(&mut self, location: StorageLocation, key: String) -> StorageReply {
        match store_scope(location) {
            None => StorageReply::Done(storage_outcome(self.local.get(&key))),
            Some(scope) => self.store_request(|app| StoreOp::Get { app, scope, key }),
        }
    }
    fn list(&mut self, location: StorageLocation, prefix: String, after: String) -> StorageReply {
        match store_scope(location) {
            None => StorageReply::Done(storage_outcome(self.local.list(&prefix, &after))),
            Some(scope) => self.store_request(|app| StoreOp::List {
                app,
                scope,
                prefix,
                after,
            }),
        }
    }
    fn set(
        &mut self,
        location: StorageLocation,
        key: String,
        value: Vec<u8>,
        expect: Option<u64>,
    ) -> StorageReply {
        match store_scope(location) {
            None => StorageReply::Done(storage_outcome(self.local.set(&key, &value, expect))),
            Some(scope) => self.store_request(|app| StoreOp::Set {
                app,
                scope,
                key,
                value,
                expect,
            }),
        }
    }
    fn remove(&mut self, location: StorageLocation, key: String, expect: Option<u64>) -> StorageReply {
        match store_scope(location) {
            None => StorageReply::Done(storage_outcome(self.local.remove(&key, expect))),
            Some(scope) => self.store_request(|app| StoreOp::Remove {
                app,
                scope,
                key,
                expect,
            }),
        }
    }
    fn cancel(&mut self, id: u32) {
        self.pending.remove(&id);
    }
    fn watch(&mut self, on: bool) {
        if self.store_watch != on {
            self.store_watch = on;
            self.actions.push(Action::StoreWatch);
        }
    }
    fn online(&mut self) -> bool {
        self.services.store.online()
    }
    fn user(&mut self) -> Option<PbxUser> {
        self.services.store.user().map(|u| PbxUser {
            id: u.id,
            name: u.name,
            extension: u.extension,
        })
    }
}

impl api::http::Host for AppCtx {
    fn request(&mut self, method: String, url: String, headers: Vec<Header>, body: Vec<u8>) -> u32 {
        let id = self.next_request();
        let inbox = self.inbox.clone();
        let (app, generation) = (self.name.clone(), self.generation);
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            inbox.push(Inbound::Http {
                app,
                generation,
                id,
                result: Err("only http and https URLs are allowed".into()),
            });
            return id;
        }
        let done = Completion::new(move |reply: Result<crate::HttpReply, String>| {
            let result = reply.map(|r| HttpResponse {
                status: r.status,
                headers: r
                    .headers
                    .into_iter()
                    .map(|(name, value)| Header { name, value })
                    .collect(),
                body: r.body,
            });
            inbox.push(Inbound::Http {
                app,
                generation,
                id,
                result,
            })
        });
        self.services.http.request(
            HttpRequest {
                method,
                url,
                headers: headers.into_iter().map(|h| (h.name, h.value)).collect(),
                body,
                credentials: None,
            },
            done,
        );
        id
    }
    fn cancel(&mut self, id: u32) {
        self.pending.remove(&id);
    }
}

impl api::calls::Host for AppCtx {
    fn dial(
        &mut self,
        number: String,
        account: Option<u8>,
        headers: Vec<Header>,
    ) -> Result<u32, Error> {
        if number.is_empty() || number.len() > 256 {
            return Err(Error::Invalid("bad number".into()));
        }
        let command = if headers.is_empty() {
            Command::Dial {
                target: number,
                account: account.map(AccountId),
                path: None::<AudioPath>,
            }
        } else {
            Command::DialWith {
                target: number,
                account: account.map(AccountId),
                headers: headers.into_iter().map(|h| (h.name, h.value)).collect(),
            }
        };
        match self.command(command)? {
            Reply::Call(id) => Ok(id.0),
            _ => Err(Error::Failed("the phone did not start a call".into())),
        }
    }
    fn answer(&mut self, call: u32) -> Result<(), Error> {
        self.command(Command::Answer {
            call: CallId(call),
            path: None,
        })
        .map(|_| ())
    }
    fn reject(&mut self, call: u32) -> Result<(), Error> {
        self.command(Command::Reject {
            call: CallId(call),
            busy: false,
        })
        .map(|_| ())
    }
    fn hangup(&mut self, call: u32) -> Result<(), Error> {
        self.command(Command::Hangup { call: CallId(call) })
            .map(|_| ())
    }
    fn send_dtmf(&mut self, call: u32, digits: String) -> Result<(), Error> {
        if digits.len() > 64 || !digits.chars().all(|c| Key::from_dial_char(c).is_some()) {
            return Err(Error::Invalid("digits must be 0-9, *, or #".into()));
        }
        for digit in digits.chars() {
            self.command(Command::Dtmf {
                call: CallId(call),
                digit,
            })?;
        }
        Ok(())
    }
    fn list(&mut self) -> Vec<CallInfo> {
        self.store
            .get::<opendeskphone_state::Calls>()
            .calls
            .iter()
            .map(call_info)
            .collect()
    }
    fn observe(&mut self, on: bool) {
        self.observe_calls = on;
    }
    fn get_mwi(&mut self) -> Vec<Mwi> {
        self.store
            .get::<opendeskphone_state::Mwi>()
            .accounts
            .iter()
            .map(|m| Mwi {
                account: m.account.0,
                new_messages: m.new,
                old_messages: m.old,
            })
            .collect()
    }
    fn dial_voicemail(&mut self, account: Option<u8>) -> Result<(), Error> {
        self.command(Command::DialVoicemail {
            account: account.map(AccountId),
        })
        .map(|_| ())
    }
}

impl api::presence::Host for AppCtx {
    fn observe(&mut self, target: String) -> Result<(), Error> {
        if !self.presence_observed.contains(&target) && self.presence_observed.len() >= PRESENCE_MAX
        {
            return Err(Error::Failed(format!("an app may observe {PRESENCE_MAX} targets")));
        }
        self.presence_observed.insert(target);
        Ok(())
    }
    fn stop_observing(&mut self, target: String) {
        self.presence_observed.remove(&target);
    }
    fn get(&mut self, target: String) -> Option<Presence> {
        let p = self.store.get::<opendeskphone_state::Presence>();
        p.entries
            .iter()
            .find(|e| entry_matches(e, &target))
            .map(|e| presence_of(e, &target))
    }
    fn set_status(&mut self, status: api::types::PresenceStatus, text: String) -> Result<(), Error> {
        use api::types::PresenceStatus as P;
        let status = match status {
            P::Available => PresenceStatus::Available,
            P::Chat => PresenceStatus::Chat,
            P::Away => PresenceStatus::Away,
            P::Dnd => PresenceStatus::Dnd,
            P::Xa => PresenceStatus::ExtendedAway,
            P::Unavailable => PresenceStatus::Unavailable,
            P::Unknown => return Err(Error::Invalid("unknown presence status".into())),
        };
        self.command(Command::SetPresence { status, text })
            .map(|_| ())
    }
    fn get_status(&mut self) -> Presence {
        let p = self.store.get::<opendeskphone_state::Presence>();
        Presence {
            target: String::new(),
            status: presence_status(p.own_status),
            text: p.own_status_text.clone(),
            line_state: LineState::Unknown,
        }
    }
}

impl api::events::Host for AppCtx {
    fn observe(&mut self, name: String) -> Result<(), Error> {
        if !self.observed.contains(&name) && self.observed.len() >= OBSERVED_MAX {
            return Err(Error::Failed(format!("an app may observe {OBSERVED_MAX} names")));
        }
        self.observed.insert(name);
        Ok(())
    }
    fn stop_observing(&mut self, name: String) {
        self.observed.remove(&name);
    }
    fn send(&mut self, app: String, name: String, payload: String) -> Result<(), String> {
        if crate::manifest::validate_name(&app).is_err() {
            return Err("bad app name".into());
        }
        let running =
            app == self.name || self.peers().installed.iter().any(|a| a.name == app && a.running);
        if !running {
            return Err(format!("the app {app:?} is not running"));
        }
        self.actions.push(Action::Send { app, name, payload });
        Ok(())
    }
}

impl api::audio::Host for AppCtx {
    fn play(&mut self, sound: Vec<u8>, start: u32) -> Result<(), Error> {
        // The audio side decodes the sound before it answers.
        let play = Command::PlayData {
            sound: opendeskphone_state::SoundData(sound),
            start_secs: start,
            app: self.name.clone(),
        };
        self.wait_for(play, Duration::from_secs(5))
            .map(|_| self.playing = true)
    }
    fn stop(&mut self) {
        // Only the app's sound: what another app plays, and a ringtone
        // preview, are not this app's to stop.
        if self.playing {
            let _ = self.store.post(Command::StopSound);
        }
    }
}

impl api::l10n::Host for AppCtx {
    fn locale(&mut self) -> String {
        "en_US".into()
    }
    fn common(&mut self, key: api::l10n::CommonText) -> String {
        use api::l10n::CommonText as C;
        match key {
            C::Ok => "OK",
            C::Cancel => "Cancel",
            C::Back => "Back",
            C::Exit => "Exit",
            C::Select => "Select",
            C::Yes => "Yes",
            C::No => "No",
            C::More => "More",
            C::Delete => "Delete",
            C::Save => "Save",
            C::Loading => "Loading...",
            C::Error => "Error",
        }
        .to_string()
    }
    fn format_time(&mut self, unix: u64, style: api::l10n::TimeStyle) -> String {
        let c = &self.services.config;
        format_time(unix, c.utc_offset_secs(), c.clock_24h(), style)
    }
    fn utc_offset(&mut self) -> i32 {
        self.services.config.utc_offset_secs()
    }
}

impl api::hotdesk::Host for AppCtx {
    fn login(&mut self, extension: String, pin: String) -> Result<(), Error> {
        if extension.is_empty() || extension.len() > 64 || pin.len() > 64 {
            return Err(Error::Invalid("bad extension or PIN".into()));
        }
        self.command(Command::SelectUser { id: extension, pin })
            .map(|_| self.hotdesk_pending = true)
    }
    fn logout(&mut self) -> Result<(), Error> {
        // An empty user means "the phone's own, MAC-keyed configuration".
        self.command(Command::SelectUser {
            id: String::new(),
            pin: String::new(),
        })
        .map(|_| self.hotdesk_pending = true)
    }
    fn current_user(&mut self) -> String {
        self.store
            .get::<opendeskphone_state::Provisioning>()
            .assigned_user
            .clone()
    }
}

impl api::diagnostics::Host for AppCtx {
    fn health(&mut self) -> Vec<api::diagnostics::SubsystemHealth> {
        use api::diagnostics::{HealthState as H, Subsystem as W, SubsystemHealth};
        use opendeskphone_state::{HealthStatus, Subsystem};
        let health = self.store.get::<opendeskphone_state::Health>();
        Subsystem::ALL
            .iter()
            .map(|s| SubsystemHealth {
                subsystem: match s {
                    Subsystem::Core => W::Core,
                    Subsystem::Config => W::Configuration,
                    Subsystem::Net => W::Network,
                    Subsystem::Sip => W::Sip,
                    Subsystem::Media => W::Media,
                    Subsystem::Dpma => W::Dpma,
                    Subsystem::Ui => W::Ui,
                    Subsystem::Apps => W::Apps,
                    Subsystem::Web => W::Web,
                    Subsystem::Ssh => W::Ssh,
                    Subsystem::Update => W::Update,
                },
                health: match health.status(*s) {
                    HealthStatus::Stopped => H::Stopped,
                    HealthStatus::Starting => H::Starting,
                    HealthStatus::Ok => H::Ok,
                    HealthStatus::Degraded(e) => H::Degraded(e.clone()),
                    HealthStatus::Failed(e) => H::Failed(e.clone()),
                },
            })
            .collect()
    }
    fn log_tail(&mut self, max: u32) -> Vec<String> {
        self.store
            .log()
            .tail(max.min(100) as usize)
            .into_iter()
            .rev()
            .map(|e| format!("{} {}", e.target, e.message))
            .collect()
    }
}

impl api::lamps::Host for AppCtx {
    fn list(&mut self) -> Vec<api::lamps::Lamp> {
        Lamp::for_model(self.store.model())
            .into_iter()
            .map(wit_lamp)
            .collect()
    }
    fn set(&mut self, lamp: api::lamps::Lamp, color: api::lamps::Color) {
        use api::lamps::Color;
        let color = match color {
            Color::Off => LampColor::Off,
            Color::Red => LampColor::Red,
            Color::Green => LampColor::Green,
            Color::Amber => LampColor::Amber,
        };
        let lamps = Lamp::for_model(self.store.model());
        if let Some(lamp) = lamps.into_iter().find(|l| wit_lamp(*l) == lamp) {
            self.window.set_lamp(lamp, color);
        }
    }
    fn release(&mut self) {
        self.window.release_lamps();
    }
}

impl api::input::Host for AppCtx {
    fn capture(&mut self, on: bool) {
        self.window.capture_input(on);
    }
}

impl AppCtx {
    /// A command for the alarm clock, which writes the alarms to the
    /// flash before it answers.
    fn alarm_command(&mut self, c: Command) -> Result<Reply, Error> {
        self.wait_for(c, Duration::from_millis(1500))
    }
}

impl api::alarms::Host for AppCtx {
    fn list(&mut self) -> Vec<api::alarms::Alarm> {
        let alarms = self.store.get::<opendeskphone_state::Alarms>();
        alarms
            .alarms
            .iter()
            .map(|a| api::alarms::Alarm {
                id: a.id,
                hour: a.hour,
                minute: a.minute,
                days: a.days,
                tone: a.tone.clone(),
                enabled: a.enabled,
            })
            .collect()
    }
    fn set(&mut self, a: api::alarms::Alarm) -> Result<u32, Error> {
        let alarm = opendeskphone_state::Alarm {
            id: a.id,
            hour: a.hour,
            minute: a.minute,
            days: a.days,
            tone: a.tone,
            enabled: a.enabled,
        };
        match self.alarm_command(Command::SetAlarm { alarm })? {
            Reply::Id(id) => Ok(id as u32),
            _ => Ok(a.id),
        }
    }
    fn remove(&mut self, id: u32) -> Result<(), Error> {
        self.alarm_command(Command::RemoveAlarm { id }).map(|_| ())
    }
    fn tones(&mut self) -> Vec<String> {
        self.store.get::<opendeskphone_state::Alarms>().tones.clone()
    }
    fn preview(&mut self, tone: Option<String>) {
        let now = opendeskphone_state::clock::mono_ms();
        match tone {
            Some(name) => {
                self.preview_until_ms = now + PREVIEW_MS;
                let _ = self.store.post(Command::PlayAlarmTone { name });
            }
            // Only the preview this app started, while it sounds: another
            // app's sound is not this app's to stop.
            None if now < self.preview_until_ms => {
                self.preview_until_ms = 0;
                let _ = self.store.post(Command::StopSound);
            }
            None => {}
        }
    }
    fn next(&mut self) -> Option<(u32, u64)> {
        self.store.get::<opendeskphone_state::Alarms>().next
    }
    fn snoozed(&mut self) -> Option<(u32, u64)> {
        self.store.get::<opendeskphone_state::Alarms>().snoozed
    }
    fn dismiss(&mut self) -> Result<(), Error> {
        self.alarm_command(Command::DismissAlarm).map(|_| ())
    }
}

/// The most operations one `canvas.draw` may carry.
const CANVAS_OPS_MAX: usize = 512;
/// The most bitmap data one `canvas.draw` may carry.
const CANVAS_BITS_MAX: usize = 64 * 1024;
/// The longest text of one drawing operation, in bytes.
const CANVAS_TEXT_MAX: usize = 64;

/// Whether a drawing is within the limits of the interface.
fn check_drawing(ops: &[api::canvas::Op]) -> Result<(), String> {
    use api::canvas::Op;
    if ops.len() > CANVAS_OPS_MAX {
        return Err(format!("more than {CANVAS_OPS_MAX} operations"));
    }
    let mut bits = 0usize;
    for op in ops {
        if let Op::Text(t) = op
            && t.text.len() > CANVAS_TEXT_MAX
        {
            return Err(format!("a text of more than {CANVAS_TEXT_MAX} bytes"));
        }
        let Op::Bitmap(b) = op else { continue };
        if b.width == 0 || b.height == 0 || b.width > 256 || b.height > 256 {
            return Err("a bitmap must be 1 to 256 pixels each way".into());
        }
        if !(1..=8).contains(&b.scale) {
            return Err("a bitmap's scale must be 1 to 8".into());
        }
        if b.bits.len() != (b.width as usize).div_ceil(8) * b.height as usize {
            return Err("a bitmap's data does not match its size".into());
        }
        bits += b.bits.len();
    }
    if bits > CANVAS_BITS_MAX {
        return Err(format!("more than {CANVAS_BITS_MAX} bytes of bitmap data"));
    }
    Ok(())
}

impl api::canvas::Host for AppCtx {
    fn theme_color(&mut self, role: api::canvas::Role) -> api::canvas::Color {
        self.window.theme_color(role)
    }
    fn text_size(&mut self, font: Font, text: String) -> Size {
        let mut end = text.len().min(CANVAS_TEXT_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let (width, height) = self.window.text_size(font, &text[..end]);
        Size { width, height }
    }
    fn create(&mut self, parent: u32, bounds: Rect) -> u32 {
        let idle = parent & IDLE_BIT;
        match self.win(parent) {
            Some((win, p)) => win.create_canvas(p, bounds) | idle,
            None => 0,
        }
    }
    fn draw(&mut self, w: u32, ops: Vec<api::canvas::Op>) -> Result<(), String> {
        check_drawing(&ops)?;
        let drawn = match self.win(w) {
            Some((win, Some(id))) => win.draw(id, ops),
            _ => false,
        };
        if drawn { Ok(()) } else { Err("not a canvas".into()) }
    }
}

/// Adds the interfaces the manifest grants to a linker.  `runtime` is
/// always there; an app importing anything else it did not declare fails
/// to instantiate.
pub(crate) fn link(
    linker: &mut wasmtime::component::Linker<AppCtx>,
    manifest: &Manifest,
) -> wasmtime::Result<()> {
    use wasmtime::component::HasSelf;
    type H = HasSelf<AppCtx>;
    api::types::add_to_linker::<_, H>(linker, |c| c)?;
    api::runtime::add_to_linker::<_, H>(linker, |c| c)?;
    for cap in &manifest.capabilities {
        match cap {
            Capability::Ui => api::ui::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Timers => api::timers::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Storage => api::storage::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Config => api::config::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Pbx => api::pbx::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Http => api::http::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Calls => api::calls::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Presence => api::presence::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Events => api::events::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Audio => api::audio::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::L10n => api::l10n::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Canvas => api::canvas::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Hotdesk => api::hotdesk::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Diagnostics => api::diagnostics::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Lamps => api::lamps::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Input => api::input::add_to_linker::<_, H>(linker, |c| c)?,
            Capability::Alarms => api::alarms::add_to_linker::<_, H>(linker, |c| c)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        // 2026-11-13 22:05:00 UTC
        let t = 1_794_607_500;
        use api::l10n::TimeStyle::{Date, DateAndTime, Time};
        assert_eq!(format_time(t, 0, true, Time), "22:05");
        assert_eq!(format_time(t, 0, false, Time), "10:05 PM");
        assert_eq!(format_time(t, 0, true, Date), "Nov 13, 2026");
        assert_eq!(format_time(t, -6 * 3600, false, DateAndTime), "Nov 13, 2026 4:05 PM");
        assert_eq!(format_time(0, 0, false, DateAndTime), "Jan 1, 1970 12:00 AM");
    }

    #[test]
    fn pbx_reply_reduction() {
        let read = |r: &[u8]| pbx_outcome(Ok(r.to_vec()), |_| Some(PbxResult::Done));
        assert!(matches!(
            read(br#"{"response":{"method":"m","result":{"success":{}}}}"#),
            Ok(PbxResult::Done)
        ));
        assert!(matches!(
            read(br#"{"response":{"errors":{"error":{"code":10005,"message":"Method not found: m "}}}}"#),
            Err(PbxError::Server(e)) if e.code == 10005 && e.message == "Method not found: m"
        ));
        assert!(matches!(read(b"<html>"), Err(PbxError::Transport(_))));
        assert!(matches!(
            pbx_outcome(Err("x".into()), |_| None),
            Err(PbxError::Transport(_))
        ));
        // An answer that is not the method's.
        assert!(matches!(
            pbx_outcome(Ok(br#"{"response":{"result":{}}}"#.to_vec()), |_| None),
            Err(PbxError::Transport(_))
        ));
    }

    #[test]
    fn drawings_over_the_limits_are_refused() {
        use api::canvas::{Bitmap, Color, Op};
        let color = Color { r: 0, g: 0, b: 0 };
        let bitmap = |width, height, scale, len| {
            Op::Bitmap(Bitmap {
                x: 0,
                y: 0,
                width,
                height,
                scale,
                color,
                bits: vec![0; len],
            })
        };
        let fill = || {
            Op::Fill((
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                color,
            ))
        };
        assert!(check_drawing(&[fill(), bitmap(9, 2, 1, 4), bitmap(256, 256, 8, 8192)]).is_ok());
        assert!(check_drawing(&(0..512).map(|_| fill()).collect::<Vec<_>>()).is_ok());
        assert!(check_drawing(&(0..513).map(|_| fill()).collect::<Vec<_>>()).is_err());
        // Nine pixels a row need two bytes a row.
        assert!(check_drawing(&[bitmap(9, 2, 1, 2)]).is_err());
        assert!(check_drawing(&[bitmap(0, 2, 1, 0)]).is_err());
        assert!(check_drawing(&[bitmap(257, 1, 1, 33)]).is_err());
        assert!(check_drawing(&[bitmap(8, 1, 0, 1)]).is_err());
        assert!(check_drawing(&[bitmap(8, 1, 9, 1)]).is_err());
        // Nine full-size bitmaps are 72 KiB.
        let many: Vec<Op> = (0..9).map(|_| bitmap(256, 256, 1, 8192)).collect();
        assert!(check_drawing(&many).is_err());
    }

    #[test]
    fn a_refused_command_keeps_its_kind() {
        use opendeskphone_state::CommandError;
        assert!(matches!(
            command_error(CommandError::Invalid("no such call".into())),
            Error::Invalid(why) if why == "no such call"
        ));
        // The interface has no kind of its own for a refusal: the reason
        // is in the text.
        assert!(matches!(
            command_error(CommandError::Denied("policy".into())),
            Error::Failed(why) if why.contains("policy")
        ));
        assert!(matches!(
            command_error(CommandError::Failed("no route".into())),
            Error::Failed(why) if why == "no route"
        ));
        // The phone may still do what was asked, and the text says so.
        assert!(matches!(
            command_error(CommandError::Timeout),
            Error::Failed(why) if why.contains("may still happen")
        ));
        assert!(matches!(command_error(CommandError::Dropped), Error::Failed(_)));
    }

    #[test]
    fn the_line_lamps_are_those_of_the_line_keys() {
        use opendeskphone_state::Model;
        let lines = |m| Lamp::for_model(m).into_iter().filter(|l| matches!(l, Lamp::Line(_))).count();
        assert_eq!([lines(Model::D60), lines(Model::D62), lines(Model::D65)], [2, 2, 6]);
        assert_eq!([lines(Model::D40), lines(Model::D45), lines(Model::D50), lines(Model::D70)], [2, 2, 4, 6]);
    }

    #[test]
    fn names_with_separators_are_refused() {
        for n in ["", ".", "..", "a/b", "a\\b", "/etc/passwd"] {
            assert!(bad_name(n), "{n}");
        }
        assert!(!bad_name("settings.json"));
    }
}
