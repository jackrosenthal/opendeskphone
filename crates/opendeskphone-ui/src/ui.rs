//! The UI thread: input, navigation, rendering, lamps, and backlight.

use crate::appview::{AppEvent, AppView};
use crate::gfx::{Canvas, HAlign, IRect};
use crate::screens::app::AppScreen;
use crate::screens::calls::{CallScreen, Dialer, Purpose};
use crate::screens::{boot, idle, menus, side, Cx, Nav, Screen, Soft, Tag};
use crate::skin::Theme;
use crate::wallpaper::{self, Backdrop};
use crate::strings as tr;
use crate::touch::{Gesture, Hit, Recognizer};
use crate::widgets;
use opendeskphone_hal::{Backlight, Blinker, Display, Hal, Input, InputEvent, Lamp, Lamps, Null, BLINK_TICK_MS};
use opendeskphone_state::{
    self as ds, clock, AudioPath, BlfState, CallId, CallState, Command, CommandError, Domain, Envelope, Event,
    InputSource, Key, LampState, LineKeyBinding, Model, Notice, Phase, ProvisionPhase, Reply, Store,
    Subsystem, Topic, TopicSet, TouchPhase, UpdatePhase, VolumeTarget,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::dialplan::DialPlan;

/// What a softkey of the idle screen does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdleSoftkey {
    /// The call log; labeled "Missed Calls" while there are unseen ones.
    CallLog,
    Contacts,
    /// Phone information.
    Status,
    Menu,
    /// The app launcher.
    Apps,
    /// Switches Do Not Disturb.
    Dnd,
    /// The call forwarding menu.
    Forward,
    Redial,
    /// The theme selector; not shown on a gray model, which has no
    /// themes.
    Theme,
    /// Opens the app of this name, and is labeled with its display name;
    /// not shown while the phone has no such app (absent, or switched
    /// off).
    App(String),
}

/// Settings the integrator maps from the merged configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiConfig {
    /// 24-hour clock.
    pub time_24h: bool,
    /// Local time offset from UTC in seconds (the UI has no zone database).
    pub utc_offset_secs: i32,
    /// Backlight brightness 0..=10 (stock `brightness`).
    pub brightness: u8,
    /// Dim the backlight when idle (stock `dim_backlight`).  As in stock,
    /// the backlight is never switched off by the timeout: without
    /// dimming it stays at `brightness`.
    pub dim_backlight: bool,
    /// The brightness level 0..=10 the backlight dims to (stock
    /// `backlight_dim_level`, 2 unless set; 0 is off).
    pub backlight_dim_level: u8,
    /// Seconds of inactivity before the backlight dims; 0 keeps it on
    /// (stock `backlight_timeout`).
    pub backlight_timeout_secs: u32,
    /// Gray panels: contrast level (stock `contrast`).
    pub contrast: u8,
    /// Highest contrast level; filled in by the UI from the HAL.
    pub contrast_max: u8,
    /// Name of the selected ringtone and the names available.
    pub ringtone: String,
    pub ringtones: Vec<String>,
    /// Off-hook dialing: milliseconds after the last digit before the
    /// number is dialed when the dial plan has not dialed it.
    pub dial_timeout_ms: u32,
    /// Off-hook dialing: each account's dial plan, by the account's
    /// configuration id (stock `dial_plan`, or `default_dial_plan` for an
    /// account without one); `None` when it does not compile.
    pub dial_plans: Vec<(String, Option<DialPlan>)>,
    /// The dial plan when the dialer has no account (stock
    /// `default_dial_plan`).
    pub default_dial_plan: Option<DialPlan>,
    /// Seconds each app's idle view shows before the next takes its turn
    /// (stock `idle_cycle_seconds`: 10 by default); 0 shows none.
    pub idle_cycle_secs: u32,
    /// The idle screen's softkeys, in order.
    pub idle_softkeys: Vec<IdleSoftkey>,
    /// The theme of the color skin, by its configuration name (the
    /// [`crate::wallpaper::Wallpaper::id`] of a built-in wallpaper); an
    /// empty or unknown name is the default theme.  Its wallpaper is
    /// behind the idle and call screens, and every screen is drawn in its
    /// palette.
    pub theme: String,
    /// A wallpaper picture from the provisioning server, shown instead of
    /// the built-in one, with its palette (the skin's own if it has none).
    pub wallpaper_picture: Option<Arc<wallpaper::Picture>>,
    /// The provisioning server fixed the theme: the phone's selector
    /// shows the choices and changes nothing.
    pub theme_locked: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            time_24h: false,
            utc_offset_secs: 0,
            brightness: 5,
            dim_backlight: true,
            backlight_dim_level: 2,
            backlight_timeout_secs: 60,
            contrast: 0,
            contrast_max: 0,
            ringtone: String::new(),
            ringtones: Vec::new(),
            dial_timeout_ms: 4000,
            dial_plans: Vec::new(),
            default_dial_plan: None,
            idle_cycle_secs: 10,
            idle_softkeys: {
                use IdleSoftkey::*;
                vec![CallLog, Contacts, Status, Menu, Apps, Dnd, Forward]
            },
            theme: String::new(),
            wallpaper_picture: None,
            theme_locked: false,
        }
    }
}

/// The theme the configuration asks for: the model's skin, and on the
/// color skin the palette of the wallpaper in use (a wallpaper picture's
/// own palette, or the skin's when it came with none).
fn configured_theme(model: opendeskphone_state::Model, cfg: &UiConfig) -> &'static Theme {
    let skin = Theme::for_model(model);
    if skin.gray {
        return skin;
    }
    match &cfg.wallpaper_picture {
        Some(picture) => picture.palette.as_ref().map_or(skin, |p| skin.in_palette(p)),
        None => skin.in_wallpaper(wallpaper::find_or_default(&cfg.theme)),
    }
}

enum Msg {
    Input(InputEvent),
    State,
    Command(Envelope),
    OpenApp(AppView),
    IdleView(AppView),
    NoIdleView(String),
    CloseApp(String),
    Config(UiConfig),
    Wake,
    /// Capture every screen through the display ([`UiHandle::capture_screens`]).
    Capture,
    Quit,
}

/// What a drawing of the screen is for ([`Ui::draw`]).
#[derive(Clone, Copy)]
enum Pass {
    /// Over black and over white in place of the wallpaper: the two
    /// halves of a [`crate::gfx::Overlay`].
    OverBlack,
    OverWhite,
    /// The picture that is shown.
    Shown,
}

/// A wallpaper in motion on the main screen.
#[derive(Clone, Copy)]
struct Moving {
    wallpaper: &'static wallpaper::Wallpaper,
    /// The time from one frame to the next, in milliseconds.
    every: u64,
    /// The number of the frame on the screen
    /// ([`wallpaper::Changes::serial`]).
    serial: u64,
}

/// What the frames of a wallpaper's motion cost: the log gets one line
/// for a wallpaper, when it has moved for [`MotionCost::FRAMES`] frames
/// after its first, and another only after another wallpaper has moved.
#[derive(Default)]
struct MotionCost {
    id: &'static str,
    /// The time the first frame took: the screen drawn whole, with this
    /// wallpaper in place of another.
    first: Duration,
    /// The frames since the first.
    frames: u32,
    /// When the first frame began, and the last, in monotonic
    /// milliseconds; the longest time from one frame to the next, apart
    /// from the time from the first to the second (the first's own).
    since: u64,
    last: u64,
    gap: u64,
    /// The frames made from the frame before ([`Ui::motion_frame`]): the
    /// time they took, the longest, and the part of it in which the
    /// wallpaper drew its frames.
    made: u32,
    spent: Duration,
    longest: Duration,
    drawing: Duration,
    /// The frames for which the screen was drawn whole (something else on
    /// it changed): the time they took, and the longest.
    whole: u32,
    whole_spent: Duration,
    whole_longest: Duration,
}

impl MotionCost {
    const FRAMES: u32 = 200;

    /// One more frame of wallpaper `id`, begun at `started`, took `time`:
    /// made from the frame before (`drawing` of the time for the
    /// wallpaper to draw its frame, the rest to bring what the frame
    /// changed to the display), or, with `drawing` `None`, with the screen
    /// drawn whole.
    fn frame(&mut self, id: &'static str, every: u64, started: u64, drawing: Option<Duration>, time: Duration) {
        if self.id != id {
            *self = MotionCost { id, first: time, since: started, last: started, ..MotionCost::default() };
            return;
        }
        if self.frames >= Self::FRAMES {
            return;
        }
        self.frames += 1;
        if self.frames > 1 {
            self.gap = self.gap.max(started - self.last);
        }
        self.last = started;
        match drawing {
            Some(drawing) => {
                self.made += 1;
                self.spent += time;
                self.drawing += drawing;
                self.longest = self.longest.max(time);
            }
            None => {
                self.whole += 1;
                self.whole_spent += time;
                self.whole_longest = self.whole_longest.max(time);
            }
        }
        if self.frames == Self::FRAMES {
            let each = |time: Duration, n: u32| time.as_micros() / u128::from(n.max(1));
            log::info!(
                "wallpaper {id}: the first frame took {} us; then {} frames, one every {} ms (asked: {every} ms), at most {} ms apart; {} made from the frame before, which took {} us each ({} us of them to draw the wallpaper), the longest {} us; {} with the screen drawn whole, which took {} us each, the longest {} us",
                self.first.as_micros(),
                self.frames,
                (started - self.since) / u64::from(self.frames),
                self.gap,
                self.made,
                each(self.spent, self.made),
                each(self.drawing, self.made),
                self.longest.as_micros(),
                self.whole,
                each(self.whole_spent, self.whole),
                self.whole_longest.as_micros(),
            );
        }
    }
}

/// Thread-safe access to a running [`Ui`].
#[derive(Clone)]
pub struct UiHandle {
    tx: Sender<Msg>,
    /// The UI's theme, as it is now.
    theme: crate::appview::ThemeCell,
}

impl UiHandle {
    /// Asks the UI to read every screen back through its display
    /// ([`Display::capture`]), on the UI's thread, when it next has a
    /// moment.  For a display that keeps a copy of what is shown for
    /// others to look at, and takes the copy when it is asked to.
    pub fn capture_screens(&self) {
        let _ = self.tx.send(Msg::Capture);
    }

    /// A view for the app `name` on this UI: [`AppView::new`], with the
    /// UI's theme from the start.  Its [`AppView::theme_color`] answers
    /// in the palette in use before the view is first on screen (an app
    /// asks for its colors as it starts), and goes on following the
    /// theme while the view is not on screen.
    pub fn new_view(&self, name: &str, model: Model) -> (AppView, crate::AppEvents) {
        AppView::with_theme(name, model, self.theme.clone())
    }

    /// Puts an app's view on top of the navigation stack (or raises it if
    /// already there).  The app gets `AppEvent::Foreground`.
    pub fn open_app(&self, view: AppView) {
        let _ = self.tx.send(Msg::OpenApp(view));
    }
    /// Offers an app's view for the idle screen's app area (stock
    /// `hasIdle` apps).  The view is display-only: it gets no keys.  Its
    /// size is [`crate::idle_app_area`].  As on stock, one view shows at
    /// a time; views that have at least one widget take turns
    /// ([`UiConfig::idle_cycle_secs`]), and a view gets
    /// `AppEvent::Foreground` when it comes on screen and
    /// `AppEvent::Background` when it leaves or the idle screen is covered.
    pub fn add_idle_view(&self, view: AppView) {
        let _ = self.tx.send(Msg::IdleView(view));
    }
    /// Withdraws the idle view of the app `name`.
    pub fn remove_idle_view(&self, name: &str) {
        let _ = self.tx.send(Msg::NoIdleView(name.to_string()));
    }
    /// Removes an app's view from the stack.
    pub fn close_app(&self, name: &str) {
        let _ = self.tx.send(Msg::CloseApp(name.to_string()));
    }
    /// Replaces the UI configuration.
    pub fn set_config(&self, cfg: UiConfig) {
        let _ = self.tx.send(Msg::Config(cfg));
    }
    /// Requests a repaint.
    pub fn wake(&self) {
        let _ = self.tx.send(Msg::Wake);
    }
    /// Makes [`Ui::run`] return.
    pub fn quit(&self) {
        let _ = self.tx.send(Msg::Quit);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BaseKind {
    Boot,
    Recovery,
    Provision,
    /// An image is being received or written.
    Update,
    /// The trial cover: the new image runs and its trial is not yet
    /// confirmed.  The same screen as [`BaseKind::Update`], in place of
    /// the idle screen or the setup screens.
    Finishing,
    Idle,
}

/// The longest the trial cover stays up, counted from when the UI first
/// sees the pending trial (in practice, from when the UI starts).
///
/// A trial normally ends within seconds of the phone having its network:
/// the main role confirms it once it is healthy.  One that does not is
/// ended by the init role, which restarts the phone when a trial boot is
/// not healthy after its `TRIAL_HEALTHY_SECS`, and rolls back after the
/// trial's boots are used up.  This is that same time (a test in `dphone`
/// holds the two together), so the cover outlasts a trial only when
/// nothing is going to end it (the rollback failed and the phone stays on
/// this image, or the record could not be removed).  The phone is then
/// usable as it is, and the web interface and the reset menu still show
/// the trial.
pub const TRIAL_COVER_MAX_SECS: u64 = 300;

/// The base screen the state calls for, the trial cover aside.
fn base_kind(st: &ds::State) -> BaseKind {
    if !matches!(st.update.phase, UpdatePhase::Idle | UpdatePhase::Failed(_)) {
        return BaseKind::Update;
    }
    match st.system.phase {
        Phase::Recovery => BaseKind::Recovery,
        Phase::Starting | Phase::NetworkWait | Phase::ShuttingDown => BaseKind::Boot,
        Phase::Provisioning => match st.provisioning.phase {
            ProvisionPhase::Provisioned | ProvisionPhase::Standalone => BaseKind::Idle,
            _ => BaseKind::Provision,
        },
        Phase::Running => BaseKind::Idle,
    }
}

fn make_base(kind: BaseKind) -> Box<dyn Screen> {
    match kind {
        BaseKind::Boot => Box::new(boot::Boot::default()),
        BaseKind::Recovery => Box::new(boot::Recovery),
        BaseKind::Provision => Box::new(boot::Provision::default()),
        BaseKind::Update => Box::new(boot::UpdateScreen { finishing: false }),
        BaseKind::Finishing => Box::new(boot::UpdateScreen { finishing: true }),
        BaseKind::Idle => Box::new(idle::Idle),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light {
    On,
    Dim,
}

struct Popup {
    until_ms: u64,
}

/// The navigation stack.
struct Stack {
    kind: BaseKind,
    base: Box<dyn Screen>,
    stack: Vec<Box<dyn Screen>>,
    soft_page: u8,
    seen_ringing: Vec<CallId>,
    /// The ringing alarm whose screen was put up.
    alarm_shown: Option<u32>,
    /// When the trial cover ends at the latest (monotonic milliseconds);
    /// set when a pending trial is first seen.
    cover_until: Option<u64>,
}

/// The four softkey positions of a native screen: action id (0: none) and
/// label.  More than four actions page three at a time behind "More".
fn soft_slots(keys: &[Soft], page: u8) -> [(u32, String); 4] {
    const MORE: u32 = u32::MAX;
    let mut out = [(0, String::new()), (0, String::new()), (0, String::new()), (0, String::new())];
    if keys.len() <= 4 {
        for (i, k) in keys.iter().enumerate() {
            out[i] = k.clone();
        }
    } else {
        let pages = keys.len().div_ceil(3);
        let page = page as usize % pages;
        for (i, k) in keys.iter().skip(page * 3).take(3).enumerate() {
            out[i] = k.clone();
        }
        out[3] = (MORE, tr::MORE.to_string());
    }
    out
}

/// The softkey buttons of a native screen on the touch skin: action id
/// and label, by position ([`Theme::soft_button`]).  Up to four actions
/// keep their positions (a screen puts "Back" at the right), blanks and
/// all.  More are shown without their blanks, all of them when they fit
/// in two rows; beyond that, seven at a time and "More", which pages.
fn touch_soft_slots(keys: &[Soft], page: u8) -> Vec<Soft> {
    if keys.len() <= 4 {
        return keys.to_vec();
    }
    let keys: Vec<Soft> = keys.iter().filter(|(id, label)| *id != 0 && !label.is_empty()).cloned().collect();
    if keys.len() <= 8 {
        return keys;
    }
    let page = page as usize % keys.len().div_ceil(7);
    let mut out: Vec<Soft> = keys.into_iter().skip(page * 7).take(7).collect();
    // "More" is the last button of the second row.
    out.resize(7, (0, String::new()));
    out.push((u32::MAX, tr::MORE.to_string()));
    out
}

/// How many pages [`touch_soft_slots`] makes of these softkeys.
fn touch_soft_pages(keys: &[Soft]) -> usize {
    let n = keys.iter().filter(|(id, label)| *id != 0 && !label.is_empty()).count();
    if n <= 8 { 1 } else { n.div_ceil(7) }
}

impl Stack {
    fn new(st: &ds::State, now_ms: u64) -> Stack {
        let mut s = Stack {
            kind: BaseKind::Boot,
            base: make_base(BaseKind::Boot),
            stack: Vec::new(),
            soft_page: 0,
            seen_ringing: Vec::new(),
            alarm_shown: None,
            cover_until: None,
        };
        s.kind = s.wanted(st, now_ms);
        s.base = make_base(s.kind);
        s
    }

    /// The base screen for the state.  While a firmware trial is pending,
    /// the cover stands in for the idle screen and for the setup screens
    /// (which come up when the trial is confirmed), for at most
    /// [`TRIAL_COVER_MAX_SECS`].  The start-up screen is not covered: it
    /// says what the phone is waiting for and leads to the network
    /// settings, which a trial that cannot get its network needs.
    fn wanted(&mut self, st: &ds::State, now_ms: u64) -> BaseKind {
        let kind = base_kind(st);
        let covered = matches!(st.update.trial, ds::TrialBoot::Pending { .. })
            && now_ms < *self.cover_until.get_or_insert(now_ms + TRIAL_COVER_MAX_SECS * 1000);
        if covered && matches!(kind, BaseKind::Idle | BaseKind::Provision) { BaseKind::Finishing } else { kind }
    }

    /// Whether going off-hook with no call opens the dialer: on the idle
    /// screen, and on the trial cover when the idle screen is what it
    /// covers (the cover never stands between the user and a call).
    fn dial_base(&self, st: &ds::State) -> bool {
        self.kind == BaseKind::Idle || (self.kind == BaseKind::Finishing && base_kind(st) == BaseKind::Idle)
    }

    fn top(&mut self) -> &mut Box<dyn Screen> {
        self.stack.last_mut().unwrap_or(&mut self.base)
    }
    fn top_ref(&self) -> &dyn Screen {
        self.stack.last().unwrap_or(&self.base).as_ref()
    }

    fn push(&mut self, cx: &Cx, s: Box<dyn Screen>) {
        self.top().leave(cx);
        self.stack.push(s);
        self.soft_page = 0;
        self.top().enter(cx);
    }
    fn pop(&mut self, cx: &Cx) {
        if let Some(mut s) = self.stack.pop() {
            s.leave(cx);
            self.soft_page = 0;
            self.top().enter(cx);
        }
    }
    /// Removes the screens above index `len` (top first).
    fn truncate(&mut self, cx: &Cx, len: usize) {
        if self.stack.len() <= len {
            return;
        }
        while self.stack.len() > len {
            if let Some(mut s) = self.stack.pop() {
                s.leave(cx);
            }
        }
        self.soft_page = 0;
        self.top().enter(cx);
    }

    fn apply(&mut self, cx: &Cx, nav: Nav) {
        match nav {
            Nav::Ignored | Nav::Stay => {}
            Nav::Push(s) => self.push(cx, s),
            Nav::Pop => self.pop(cx),
            Nav::Home => {
                // Back to the base, but never hide a live call.
                let keep = self.stack.iter().position(|s| s.tag() == Tag::Call).map(|i| i + 1).unwrap_or(0);
                self.truncate(cx, keep);
            }
        }
    }

    fn call_screen_pos(&self) -> Option<usize> {
        self.stack.iter().position(|s| s.tag() == Tag::Call)
    }

    /// Makes the call screen the top screen.
    fn raise_call_screen(&mut self, cx: &Cx) {
        match self.call_screen_pos() {
            Some(i) => self.truncate(cx, i + 1),
            None => self.push(cx, Box::new(CallScreen::default())),
        }
    }

    /// Follows the state: base screen kind, and the call screen.
    fn sync(&mut self, cx: &Cx) {
        let kind = self.wanted(cx.st, cx.now_ms);
        if kind != self.kind {
            // The trial cover giving way to the idle screen leaves what
            // is above it (a dialer, a call) alone.
            if !(self.kind == BaseKind::Finishing && kind == BaseKind::Idle) {
                self.truncate(cx, 0);
            }
            let top = self.stack.is_empty();
            if top {
                self.base.leave(cx);
            }
            self.kind = kind;
            self.base = make_base(kind);
            if top {
                self.base.enter(cx);
            }
        }
        if kind == BaseKind::Update {
            return;
        }
        // An app's hidden call has no call screen.
        let live = cx.st.calls.visible().next().is_some();
        let ringing: Vec<CallId> = cx.st.calls.ringing().map(|c| c.id).collect();
        let new_ring = ringing.iter().any(|id| !self.seen_ringing.contains(id));
        self.seen_ringing = ringing;
        match (live, self.call_screen_pos()) {
            (true, None) => {
                // A live dialer stays on top of the new call screen only
                // if it is for a transfer or conference; a plain dialer has
                // done its job.
                if self.top_ref().tag() == Tag::Dialer {
                    self.pop(cx);
                }
                self.push(cx, Box::new(CallScreen::default()));
            }
            (true, Some(i)) => {
                if new_ring && self.stack.len() > i + 1 && self.top_ref().tag() != Tag::Dialer {
                    self.truncate(cx, i + 1);
                }
            }
            (false, Some(i)) => self.truncate(cx, i),
            (false, None) => {}
        }
        self.sync_alarm(cx);
    }

    /// The ringing alarm's screen: put over whatever is on screen when an
    /// alarm begins to ring, and taken away when it stops.  A user who
    /// leaves it is not brought back to it.
    fn sync_alarm(&mut self, cx: &Cx) {
        let ringing = cx.st.alarms.ringing.as_ref().map(|r| r.id);
        if ringing == self.alarm_shown {
            return;
        }
        self.alarm_shown = ringing;
        let pos = self.stack.iter().position(|s| s.tag() == Tag::Alarm);
        match (ringing, pos) {
            (Some(_), None) => self.push(cx, Box::new(crate::screens::alarm::AlarmScreen)),
            (None, Some(i)) => {
                let was_top = i + 1 == self.stack.len();
                let mut s = self.stack.remove(i);
                s.leave(cx);
                if was_top {
                    self.soft_page = 0;
                    self.top().enter(cx);
                }
            }
            _ => {}
        }
    }

    fn open_app(&mut self, cx: &Cx, view: AppView) {
        if let Some(i) = self.stack.iter().position(|s| s.app_name() == Some(view.name())) {
            let mut s = self.stack.remove(i);
            s.leave(cx);
        }
        self.push(cx, Box::new(AppScreen::new(view)));
    }

    fn close_app(&mut self, cx: &Cx, name: &str) {
        if let Some(i) = self.stack.iter().position(|s| s.app_name() == Some(name)) {
            let was_top = i + 1 == self.stack.len();
            let mut s = self.stack.remove(i);
            s.leave(cx);
            if was_top {
                self.soft_page = 0;
                self.top().enter(cx);
            }
        }
    }

    fn blf_press(&mut self, cx: &Cx, entry: &str, number: &str) {
        let e = cx.st.presence.get(entry);
        // As stock: a press dials the contact whatever its state, unless
        // the contact is one whose ringing calls this phone may pick up.
        if e.is_some_and(|e| e.pickup_ringing && matches!(e.state, BlfState::Ringing | BlfState::OnHoldRinging)) {
            cx.post(Command::Pickup { blf_entry: entry.to_string() });
            return;
        }
        let number = if number.is_empty() { e.map(|e| e.number.as_str()).unwrap_or("") } else { number };
        if !number.is_empty() {
            cx.post(Command::Dial { target: number.to_string(), account: e.and_then(|e| e.account), path: None });
        }
    }

    /// Keys no screen took.
    fn global_key(&mut self, cx: &Cx, key: Key) {
        let st = cx.st;
        let calls = &st.calls;
        let live = !calls.is_idle();
        let ringing = calls.ringing().next().map(|c| c.id);
        let focused = calls.focused().filter(|c| !c.state.is_ended()).or_else(|| calls.live().next());
        let idle_base = self.kind == BaseKind::Idle;
        let dial_base = self.dial_base(st);
        match key {
            Key::Mute if live => cx.post(Command::SetMute { on: !st.audio.muted }),
            Key::Speaker | Key::Headset => {
                let path = if key == Key::Speaker { AudioPath::Speaker } else { AudioPath::Headset };
                if let Some(call) = ringing {
                    cx.post(Command::Answer { call, path: Some(path) });
                } else if let (true, Some(call)) = (live, focused) {
                    if st.audio.path == path {
                        // Leaving the path: back to the handset if it is
                        // lifted, else the call ends.
                        if cx.off_hook {
                            cx.post(Command::SetAudioPath { path: AudioPath::Handset });
                        } else {
                            cx.post(Command::Hangup { call: call.id });
                        }
                    } else {
                        cx.post(Command::SetAudioPath { path });
                    }
                } else if dial_base {
                    // Either key alone goes off-hook on its path, as on
                    // the stock firmware (observed on a D60: the Headset
                    // key at idle opens New Call, attaches the headset,
                    // and plays the dial tone there).
                    let d = Dialer::off_hook(cx, path, None);
                    self.push(cx, Box::new(d));
                } else if key == Key::Headset {
                    cx.post(Command::SetHeadsetMode { on: !st.audio.headset_mode });
                }
            }
            Key::Hold | Key::Transfer | Key::Conference if live => {
                if self.top_ref().tag() != Tag::Call {
                    self.raise_call_screen(cx);
                    let nav = self.top().key(cx, key);
                    self.apply(cx, nav);
                }
            }
            Key::Redial if !live => cx.post(Command::Redial),
            Key::Messages => {
                if st.apps.installed.iter().any(|a| a.name == "voicemail") {
                    cx.post(Command::LaunchApp { name: "voicemail".into() });
                } else {
                    cx.post(Command::DialVoicemail { account: None });
                }
            }
            Key::Line(n) => {
                let Some(k) = st.keys.line_keys.iter().find(|k| k.index + 1 == n) else {
                    // No key table yet: line 1 goes home.
                    if n == 1 {
                        self.apply(cx, Nav::Home);
                    }
                    return;
                };
                match &k.binding {
                    LineKeyBinding::Account(a) => {
                        if let Some(call) = k.calls.first() {
                            let c = calls.get(*call);
                            match c.map(|c| &c.state) {
                                Some(CallState::Ringing) => {
                                    cx.post(Command::Answer { call: *call, path: cx.off_hook.then_some(AudioPath::Handset) })
                                }
                                Some(CallState::Held { .. }) => cx.post(Command::Resume { call: *call }),
                                _ => cx.post(Command::Focus { call: *call }),
                            }
                            self.raise_call_screen(cx);
                        } else if dial_base && !live {
                            let path = if cx.off_hook {
                                AudioPath::Handset
                            } else if st.audio.headset_mode {
                                AudioPath::Headset
                            } else {
                                AudioPath::Speaker
                            };
                            let d = Dialer::off_hook(cx, path, Some(*a));
                            self.apply(cx, Nav::Home);
                            self.push(cx, Box::new(d));
                        } else if live {
                            let mut d = Dialer::new(Purpose::Conference(focused.map(|c| c.id).unwrap_or(CallId(0))), Some(*a));
                            d.text.clear();
                            self.push(cx, Box::new(d));
                        }
                    }
                    LineKeyBinding::Blf(entry) => self.blf_press(cx, entry, ""),
                    LineKeyBinding::Unused => {}
                }
            }
            Key::Blf(n) => {
                let page = st.keys.rapid_page;
                if let Some(k) = st.keys.rapid_dial.iter().find(|k| k.index + 1 == n && k.page == page) {
                    self.blf_press(cx, &k.entry, &k.number);
                }
            }
            Key::BlfPageLeft | Key::BlfPageRight => {
                let pages = st.keys.rapid_pages.max(1) as i32;
                let d = if key == Key::BlfPageLeft { -1 } else { 1 };
                let page = (st.keys.rapid_page as i32 + d).rem_euclid(pages) as u8;
                cx.post(Command::SetRapidDialPage { page });
            }
            Key::Menu if idle_base => self.push(cx, menus::main_menu()),
            Key::Apps if idle_base => self.push(cx, menus::launcher()),
            Key::Contacts => cx.post(Command::LaunchApp { name: "contacts".into() }),
            Key::Status | Key::Info if idle_base => self.push(cx, menus::about()),
            Key::Cancel => self.pop(cx),
            _ => {}
        }
    }

    fn hook(&mut self, cx: &Cx, off_hook: bool) {
        let nav = self.top().hook(cx, off_hook);
        if !matches!(nav, Nav::Ignored) {
            self.apply(cx, nav);
            return;
        }
        let calls = &cx.st.calls;
        if off_hook {
            if let Some(call) = calls.ringing().next() {
                cx.post(Command::Answer { call: call.id, path: Some(AudioPath::Handset) });
            } else if !calls.is_idle() {
                cx.post(Command::SetAudioPath { path: AudioPath::Handset });
            } else if self.dial_base(cx.st) {
                let d = Dialer::off_hook(cx, AudioPath::Handset, None);
                self.push(cx, Box::new(d));
            }
        } else if !calls.is_idle() {
            // Hanging up ends the call that is on the handset; calls on
            // the speaker or headset continue.
            if matches!(cx.st.audio.path, AudioPath::Handset | AudioPath::None) {
                let target = calls
                    .focused()
                    .filter(|c| !c.state.is_ended() && c.state != CallState::Ringing && !c.state.is_held())
                    .or_else(|| calls.live().find(|c| c.state != CallState::Ringing && !c.state.is_held()));
                if let Some(c) = target {
                    cx.post(Command::Hangup { call: c.id });
                }
            }
        } else if cx.st.audio.path == AudioPath::Handset {
            cx.post(Command::SetAudioPath { path: AudioPath::None });
        }
    }
}

/// The phone UI.  See the crate docs.
pub struct Ui {
    store: Store,
    model: Model,
    th: &'static Theme,
    /// `th`, for the views of apps (see [`UiHandle::new_view`]).
    theme_cell: crate::appview::ThemeCell,
    display: Box<dyn Display>,
    backlight: Box<dyn Backlight>,
    lamps: Box<dyn Lamps>,
    input: Option<Box<dyn Input>>,
    main: Canvas,
    side: Option<Canvas>,
    cfg: Arc<UiConfig>,
    nav: Stack,
    off_hook: bool,
    /// Touch: the finger on the screen.
    finger: Recognizer,
    /// What the finger went down on: the region of the picture that was
    /// on the screen then.
    touched: Option<Hit>,
    /// The finger went down on a dark screen: it lights the screen, and
    /// does nothing else until it is lifted.
    touch_wakes: bool,
    popup: Option<Popup>,
    notice_deadlines: Vec<(u64, u64)>,
    blinker: Blinker,
    light: Light,
    /// The panels are on and lit: the first frame has been drawn.  Until
    /// then nothing is written to the panels or the backlight.
    lit: bool,
    level: u8,
    /// The main screen's backlight gives light (it is lit, or dimmed to a
    /// level above none).
    backlight_on: bool,
    last_activity_ms: u64,
    fixed_time: Option<(u64, u64)>,
    /// The time the wallpaper's motion is drawn for, when a test sets it.
    motion_time: Option<u64>,
    /// When the next frame of the wallpaper's motion is due; `None` while
    /// nothing on screen moves.
    motion_deadline: Option<u64>,
    /// That time has come.
    motion_due: bool,
    /// How many frames of the motion were made from the frame before
    /// ([`Ui::motion_frame`]).
    motion_frames: u64,
    /// The wallpaper in motion on the main screen, as last drawn; `None`
    /// while nothing on screen moves.
    moving: Option<Moving>,
    /// What the screen shown draws over that wallpaper.
    over: crate::gfx::Overlay,
    /// What the frames of the motion cost, for the log.
    motion_cost: MotionCost,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    commands: Option<ds::CommandReceiver>,
    state_pending: Arc<AtomicBool>,
    _guard: ds::CallbackGuard,
    needs_draw: bool,
    quit: bool,
    last_failed_update: bool,
    /// When the top screen next needs a tick or repaint (set at each draw).
    screen_deadline: Option<u64>,
    /// The apps' views for the idle screen (`hasIdle`), least recently
    /// shown first.
    idle_views: Vec<AppView>,
    /// The one whose turn it is.
    idle_view: Option<AppView>,
    /// The idle view was told it is on screen.
    idle_shown: bool,
    /// When the next view takes its turn.
    idle_turn_ms: u64,
}

impl Ui {
    /// Builds the UI from the HAL bundle, taking the display, backlight,
    /// input, and lamps out of it (null devices are left in their place).
    pub fn from_hal(hal: &mut Hal, store: Store, cfg: UiConfig) -> Ui {
        let display = std::mem::replace(&mut hal.display, Box::new(Null("display")));
        let backlight = std::mem::replace(&mut hal.backlight, Box::new(Null("backlight")));
        let input = std::mem::replace(&mut hal.input, Box::new(Null("input")));
        let lamps = std::mem::replace(&mut hal.lamps, Box::new(Null("lamps")));
        Ui::new(display, backlight, input, lamps, store, cfg)
    }

    /// Builds the UI from its devices.  The model comes from the store.
    pub fn new(
        display: Box<dyn Display>,
        backlight: Box<dyn Backlight>,
        mut input: Box<dyn Input>,
        lamps: Box<dyn Lamps>,
        store: Store,
        mut cfg: UiConfig,
    ) -> Ui {
        let model = store.model();
        let th = configured_theme(model, &cfg);
        let size = model.main_screen();
        let main = Canvas::new(size.width, size.height, th.gray);
        let side = model.side_screen().map(|s| Canvas::new(s.width, s.height, th.gray));
        cfg.contrast_max = backlight.max_contrast(opendeskphone_hal::Screen::Main);
        let (tx, rx) = mpsc::channel();
        let state_pending = Arc::new(AtomicBool::new(false));
        // Everything except our own topic and the log.
        let topics: TopicSet = Topic::ALL.into_iter().filter(|t| !matches!(t, Topic::Ui | Topic::Log)).collect();
        let guard = {
            let tx = tx.clone();
            let pending = state_pending.clone();
            store.on_change(topics, move |_| {
                if !pending.swap(true, Ordering::AcqRel) {
                    let _ = tx.send(Msg::State);
                }
            })
        };
        let commands = Some(store.handle(Domain::Ui));
        let off_hook = input.off_hook().unwrap_or(false);
        let nav = Stack::new(&store.snapshot(), clock::mono_ms());
        let mut ui = Ui {
            store,
            model,
            th,
            theme_cell: Arc::new(std::sync::Mutex::new(th)),
            display,
            backlight,
            lamps,
            input: Some(input),
            main,
            side,
            cfg: Arc::new(cfg),
            nav,
            off_hook,
            finger: Recognizer::new(),
            touched: None,
            touch_wakes: false,
            popup: None,
            notice_deadlines: Vec::new(),
            blinker: Blinker::new(),
            light: Light::On,
            lit: false,
            level: 0,
            last_activity_ms: clock::mono_ms(),
            fixed_time: None,
            motion_time: None,
            motion_deadline: None,
            motion_due: false,
            motion_frames: 0,
            moving: None,
            over: crate::gfx::Overlay::default(),
            motion_cost: MotionCost::default(),
            backlight_on: true,
            tx,
            rx,
            commands,
            state_pending,
            _guard: guard,
            needs_draw: true,
            quit: false,
            last_failed_update: false,
            screen_deadline: None,
            idle_views: Vec::new(),
            idle_view: None,
            idle_shown: false,
            idle_turn_ms: 0,
        };
        // The panels and the backlight are left as they are (off, on a
        // phone that has just started) until the first frame is drawn; see
        // `light_up`.
        ui.apply_light(Light::On);
        let c = ui.cfg.contrast;
        ui.store.update::<ds::Ui, _>(|u| u.contrast = c);
        ui.store.set_health(Subsystem::Ui, ds::HealthStatus::Ok);
        ui
    }

    /// A handle for other threads.
    pub fn handle(&self) -> UiHandle {
        UiHandle { tx: self.tx.clone(), theme: self.theme_cell.clone() }
    }

    /// Freezes the clock the screens see (snapshot tests): monotonic
    /// milliseconds and Unix seconds.
    pub fn set_fixed_time(&mut self, time: Option<(u64, u64)>) {
        self.fixed_time = time;
        self.needs_draw = true;
    }

    /// Draws the wallpaper's motion as it is at `ms` (of any clock), or,
    /// with `None`, as the time has it.  While the time is fixed
    /// ([`Ui::set_fixed_time`]) and none is set here, nothing moves: the
    /// wallpaper is its still picture.
    pub fn set_motion_time(&mut self, ms: Option<u64>) {
        self.motion_time = ms;
        self.needs_draw = true;
    }

    /// When the wallpaper's next frame is due, if it is moving (tests).
    pub fn motion_deadline(&self) -> Option<u64> {
        self.motion_deadline
    }

    /// How many frames of the wallpaper's motion were made from the frame
    /// before, without drawing the screen again (tests).
    pub fn motion_frames(&self) -> u64 {
        self.motion_frames
    }

    /// Shows the wallpaper's motion as it is at `ms`, as the UI does when
    /// a frame is due and nothing else on the screen has changed: only
    /// what the motion changed is drawn (tests and measurements).
    pub fn motion_step(&mut self, ms: u64) {
        self.motion_time = Some(ms);
        if self.needs_draw {
            self.draw();
        } else {
            self.motion_frame();
        }
    }

    fn now(&self) -> (u64, u64) {
        self.fixed_time.unwrap_or_else(|| (clock::mono_ms(), clock::unix_secs()))
    }

    /// Runs `f` with a screen context and the navigation stack.
    fn with_cx<R>(&mut self, f: impl FnOnce(&mut Stack, &Cx) -> R) -> R {
        let st = self.store.snapshot();
        let cfg = self.cfg.clone();
        let store = self.store.clone();
        let (now_ms, unix) = self.now();
        let idle_view = self.idle_view.clone();
        let cx = Cx {
            store: &store,
            st: &st,
            th: self.th,
            model: self.model,
            cfg: &cfg,
            now_ms,
            unix,
            off_hook: self.off_hook,
            idle_view: idle_view.as_ref(),
        };
        let r = f(&mut self.nav, &cx);
        self.needs_draw = true;
        r
    }

    // ------------------------------------------------------------ light

    /// The raw level of a screen's backlight for a brightness setting,
    /// by the stock table of the model.
    fn hw_level(&self, screen: opendeskphone_hal::Screen, brightness: u8) -> u8 {
        opendeskphone_hal::backlight_level(self.model, screen, brightness).min(self.backlight.max_brightness(screen))
    }

    /// The raw level a screen's backlight has while lit: a level set by
    /// command, or the `brightness` setting's.
    fn on_level(&self, screen: opendeskphone_hal::Screen) -> u8 {
        if self.level > 0 {
            self.level.min(self.backlight.max_brightness(screen))
        } else {
            self.hw_level(screen, self.cfg.brightness)
        }
    }

    fn apply_light(&mut self, light: Light) {
        self.light = light;
        let mut main = (0, 0);
        for s in self.display.screens() {
            let on = self.on_level(s);
            let level = match light {
                Light::On => on,
                // Stock dims to the level of `backlight_dim_level`; never
                // brighter than the lit level.
                Light::Dim => self.hw_level(s, self.cfg.backlight_dim_level).min(on),
            };
            if self.lit {
                let _ = self.backlight.set_brightness(s, level);
            }
            if s == opendeskphone_hal::Screen::Main {
                main = (on, level);
            }
        }
        // The wallpaper moves only while there is light to see it by.
        if self.backlight_on != (main.1 > 0) {
            self.backlight_on = main.1 > 0;
            self.needs_draw = true;
        }
        self.store.update::<ds::Ui, _>(|u| {
            u.backlight_on = main.1 > 0;
            u.backlight_level = main.0;
        });
    }

    /// Turns the panels on, once the first frame is in their buffers, so
    /// that the first thing they show is that frame: the contrast first
    /// (the gray panels take it as they come on), then the backlight level
    /// (the kernel lights the backlight with the panel, at the level set
    /// last), then the panels themselves.
    fn light_up(&mut self) {
        self.lit = true;
        if self.cfg.contrast_max > 0 {
            let c = self.cfg.contrast.min(self.cfg.contrast_max);
            let _ = self.backlight.set_contrast(opendeskphone_hal::Screen::Main, c);
        }
        self.apply_light(self.light);
        for s in self.display.screens() {
            if let Err(e) = self.display.set_power(s, true) {
                log::warn!("display {s:?}: cannot turn the panel on: {e}");
            }
        }
    }

    fn activity(&mut self) {
        self.last_activity_ms = clock::mono_ms();
        if self.light != Light::On {
            self.apply_light(Light::On);
        }
    }

    fn light_deadline(&self) -> Option<u64> {
        let t = self.cfg.backlight_timeout_secs as u64;
        (t > 0 && self.cfg.dim_backlight && self.light == Light::On).then(|| self.last_activity_ms + t * 1000)
    }

    fn check_light(&mut self) {
        let st_calls_idle = self.store.read::<ds::Calls, _>(|c| c.is_idle());
        if !st_calls_idle {
            self.activity();
            return;
        }
        if let Some(d) = self.light_deadline()
            && clock::mono_ms() >= d {
                self.apply_light(Light::Dim);
            }
    }

    // ------------------------------------------------------------ input

    /// Feeds a key event as if from the keypad.
    pub fn key(&mut self, key: Key, down: bool, source: InputSource) {
        self.store.emit(Event::Key { key, down, source });
        if !down {
            return;
        }
        self.activity();
        if self.nav.kind == BaseKind::Update {
            return;
        }
        // A notice on screen takes Select, Cancel, and the softkeys.
        if matches!(key, Key::Select | Key::Cancel | Key::Soft(_)) {
            let top = self.store.read::<ds::Ui, _>(|u| u.notices.last().map(|n| n.id));
            if let Some(id) = top {
                self.dismiss_notice(id);
                return;
            }
        }
        let captures = self.nav.top_ref().captures_all_keys();
        // Any other key takes the volume popup down, as on stock (whose
        // `phone` hides it, and stops the ringer it sounded, on any event
        // but a volume key; the audio subsystem stops the ringer here).
        if self.popup.is_some() && !matches!(key, Key::VolumeUp | Key::VolumeDown) {
            self.popup = None;
            self.needs_draw = true;
        }
        if !captures && matches!(key, Key::VolumeUp | Key::VolumeDown) {
            let delta = if key == Key::VolumeUp { 1 } else { -1 };
            self.store.post(Command::AdjustVolume { target: VolumeTarget::Current, delta });
            // (Touch: long enough to reach for the popup's own buttons.)
            self.popup = Some(Popup { until_ms: self.now().0 + if self.th.touch { 4000 } else { 2000 } });
            self.needs_draw = true;
            return;
        }
        self.with_cx(|nav, cx| {
            if let (Key::Soft(n @ 1..=4), None, false) = (key, nav.top_ref().raw_softkeys(cx), captures) {
                let keys = nav.top_ref().softkeys(cx);
                let (id, _) = soft_slots(&keys, nav.soft_page)[n as usize - 1].clone();
                if id == u32::MAX {
                    nav.soft_page = (nav.soft_page + 1) % keys.len().div_ceil(3).max(1) as u8;
                } else if id != 0 {
                    let r = nav.top().softkey(cx, id);
                    nav.apply(cx, r);
                }
                return;
            }
            let r = nav.top().key(cx, key);
            if matches!(r, Nav::Ignored) {
                if !captures {
                    nav.global_key(cx, key);
                }
            } else {
                nav.apply(cx, r);
            }
        });
    }

    /// Feeds a hookswitch change.
    pub fn hook(&mut self, off_hook: bool, source: InputSource) {
        self.off_hook = off_hook;
        self.store.emit(Event::Hook { off_hook, source });
        self.activity();
        if self.popup.take().is_some() {
            // The hookswitch takes the volume popup down as a key does.
            self.needs_draw = true;
        }
        if self.nav.kind == BaseKind::Update {
            return;
        }
        self.with_cx(|nav, cx| nav.hook(cx, off_hook));
    }

    /// Feeds one event of a finger on the main screen, at pixel (`x`,
    /// `y`), as if from the touchscreen.  Only the touch skin takes it.
    ///
    /// A touch counts as activity.  A finger that comes down on a dark
    /// screen lights it and does nothing else.  Otherwise the gesture the
    /// finger makes ([`crate::touch::Recognizer`]) goes to the region it
    /// came down on, of the picture that was on the screen then
    /// ([`crate::gfx::Canvas::hit`]): a tap does what the region's
    /// [`Hit`] says, through the same paths as keys; a drag goes to the
    /// top screen.
    pub fn touch(&mut self, phase: TouchPhase, x: u16, y: u16, source: InputSource) {
        self.store.emit(Event::Touch { phase, x, y, source });
        if !self.th.touch {
            return;
        }
        let dark = !self.backlight_on;
        self.activity();
        let (x, y) = (i32::from(x), i32::from(y));
        if phase == TouchPhase::Down {
            self.touch_wakes = dark;
            self.touched = self.main.hit_at(x, y);
        }
        let gesture = self.finger.touch(phase, x, y, self.now().0);
        if let Some(g) = gesture {
            self.gesture(g, source);
        }
    }

    /// What a finger did, to the region it came down on.
    fn gesture(&mut self, gesture: Gesture, source: InputSource) {
        if self.touch_wakes || self.nav.kind == BaseKind::Update {
            return;
        }
        let hit = self.touched;
        match gesture {
            Gesture::Tap { x, y } => self.tapped(hit, x, y, false, source),
            Gesture::LongPress { x, y } => self.tapped(hit, x, y, true, source),
            // A drag is the top screen's, unless the finger came down on
            // something that is not the screen's (a softkey, a dialog).
            Gesture::Drag { x, y, dx, dy } if matches!(hit, None | Some(Hit::Row { .. } | Hit::Custom(_))) => {
                self.with_cx(|nav, cx| {
                    let mut r = nav.top().drag_at(cx, hit, x, y, dx, dy);
                    if matches!(r, Nav::Ignored)
                        && let Some((list, len)) = nav.top().list_state(cx)
                    {
                        list.drag(dy, cx.th.row_h, len);
                        r = Nav::Stay;
                    }
                    nav.apply(cx, r);
                });
            }
            Gesture::DragEnd { .. } if matches!(hit, None | Some(Hit::Row { .. } | Hit::Custom(_))) => {
                self.with_cx(|nav, cx| {
                    let r = nav.top().drag_end(cx, hit);
                    nav.apply(cx, r);
                });
            }
            Gesture::Drag { .. } | Gesture::DragEnd { .. } => {}
        }
    }

    /// A tap (or, with `long`, a long press) on a region.
    fn tapped(&mut self, hit: Option<Hit>, x: i32, y: i32, long: bool, source: InputSource) {
        match hit {
            None | Some(Hit::Nothing) => {}
            Some(Hit::Dismiss) => {
                let notice = self.store.read::<ds::Ui, _>(|u| u.notices.last().map(|n| n.id));
                match notice {
                    Some(id) => self.dismiss_notice(id),
                    None => self.popup = None,
                }
                self.needs_draw = true;
            }
            Some(Hit::Key(key)) => {
                self.key(key, true, source);
                self.key(key, false, source);
            }
            Some(Hit::Soft(id)) => self.with_cx(|nav, cx| {
                if id == u32::MAX {
                    let pages = touch_soft_pages(&nav.top_ref().softkeys(cx));
                    nav.soft_page = ((nav.soft_page as usize + 1) % pages) as u8;
                } else if id != 0 {
                    let r = nav.top().softkey(cx, id);
                    nav.apply(cx, r);
                }
            }),
            Some(hit @ (Hit::Row { .. } | Hit::Custom(_))) => {
                let select = self.with_cx(|nav, cx| {
                    let mut r = if long { nav.top().long_press(cx, hit, x, y) } else { Nav::Ignored };
                    if matches!(r, Nav::Ignored) {
                        r = nav.top().tap(cx, hit, x, y);
                    }
                    // A row of a list nobody took: selected, and then
                    // the Select key.
                    let mut select = false;
                    if let (Nav::Ignored, Hit::Row { row, .. }) = (&r, hit)
                        && let Some((list, len)) = nav.top().list_state(cx)
                        && (row as usize) < len
                    {
                        list.selected = row as usize;
                        select = true;
                    }
                    nav.apply(cx, r);
                    select
                });
                if select {
                    self.key(Key::Select, true, source);
                    self.key(Key::Select, false, source);
                }
            }
        }
    }

    fn input_event(&mut self, ev: InputEvent, source: InputSource) {
        match ev {
            InputEvent::Key { key, down } => self.key(key, down, source),
            InputEvent::Hook { off_hook } => self.hook(off_hook, source),
            InputEvent::Touch { phase, x, y } => self.touch(phase, x, y, source),
        }
    }

    // --------------------------------------------------------- commands

    fn dismiss_notice(&mut self, id: u64) {
        self.store.update::<ds::Ui, _>(|u| u.notices.retain(|n| n.id != id));
        self.notice_deadlines.retain(|(i, _)| *i != id);
        self.needs_draw = true;
    }

    fn notify(&mut self, level: ds::NoticeLevel, title: String, text: String, timeout_ms: u32) -> u64 {
        let id = self.store.next_notice_id();
        self.store.update::<ds::Ui, _>(|u| {
            u.notices.push(Notice { id, level, title, text, timeout_ms });
            // Bound the backlog.
            if u.notices.len() > 8 {
                u.notices.remove(0);
            }
        });
        if timeout_ms > 0 {
            self.notice_deadlines.push((id, self.now().0 + timeout_ms as u64));
        }
        self.activity();
        self.needs_draw = true;
        id
    }

    fn command(&mut self, env: Envelope) {
        let (cmd, responder) = env.split();
        let result = match cmd {
            Command::InjectKey { key, down } => {
                self.key(key, down, InputSource::Injected);
                Ok(Reply::Done)
            }
            Command::InjectHook { off_hook } => {
                self.hook(off_hook, InputSource::Injected);
                Ok(Reply::Done)
            }
            Command::InjectTouch { phase, x, y } => {
                if self.th.touch {
                    self.touch(phase, x, y, InputSource::Injected);
                    Ok(Reply::Done)
                } else {
                    Err(CommandError::Invalid(format!("the {} has no touchscreen", self.model)))
                }
            }
            Command::Notify { level, title, text, timeout_ms } => Ok(Reply::Id(self.notify(level, title, text, timeout_ms))),
            Command::DismissNotice { id } => {
                self.dismiss_notice(id);
                Ok(Reply::Done)
            }
            Command::SetBacklight { level } => {
                self.level = level.min(self.backlight.max_brightness(opendeskphone_hal::Screen::Main));
                self.last_activity_ms = clock::mono_ms();
                self.apply_light(Light::On);
                Ok(Reply::Done)
            }
            Command::ResetIdleTimer => {
                self.activity();
                Ok(Reply::Done)
            }
            Command::SetContrast { level } => {
                let level = level.min(self.cfg.contrast_max);
                match self.backlight.set_contrast(opendeskphone_hal::Screen::Main, level) {
                    Ok(()) => {
                        self.store.update::<ds::Ui, _>(|u| u.contrast = level);
                        Ok(Reply::Done)
                    }
                    Err(e) => Err(CommandError::Failed(e.to_string())),
                }
            }
            Command::ShowScreen { name } => {
                let screen: Option<Option<Box<dyn Screen>>> = match name.as_str() {
                    "idle" | "home" => Some(None),
                    "menu" => Some(Some(menus::main_menu())),
                    "call_log" => Some(Some(menus::call_log())),
                    "apps" => Some(Some(menus::launcher())),
                    "forward" => Some(Some(menus::forward_menu())),
                    "paging" => Some(Some(menus::paging())),
                    "bluetooth" => Some(Some(crate::screens::bluetooth::screen())),
                    "settings" => Some(Some(menus::settings())),
                    // A gray model has no themes.
                    "theme" if !self.th.gray => Some(Some(menus::themes(&self.cfg))),
                    "network" => Some(Some(menus::network_menu())),
                    "status" | "about" => Some(Some(menus::about())),
                    "dialer" => Some(Some(Box::new(Dialer::new(Purpose::Call, None)))),
                    _ => None,
                };
                match screen {
                    None => Err(CommandError::Invalid(format!("no screen named {name:?}"))),
                    Some(s) => {
                        self.activity();
                        self.with_cx(|nav, cx| {
                            nav.apply(cx, Nav::Home);
                            if let Some(s) = s {
                                nav.push(cx, s);
                            }
                        });
                        Ok(Reply::Done)
                    }
                }
            }
            other => Err(CommandError::Invalid(format!("not a UI command: {other:?}"))),
        };
        responder.reply(result);
    }

    // ------------------------------------------------------------ lamps

    fn sync_lamps(&mut self) {
        let keys = self.store.get::<ds::Keys>();
        // The lamps the top screen (an app) sets show its colors.
        let own = self.nav.top_ref().lamps();
        for lamp in self.lamps.lamps() {
            let state = match own.iter().find(|(l, _)| *l == lamp) {
                Some((_, color)) => Some(LampState::solid(*color)),
                None => match lamp {
                    Lamp::Line(i) => keys.line_keys.iter().find(|k| k.index == i).map(|k| k.lamp),
                    Lamp::Blf(i) => {
                        keys.rapid_dial.iter().find(|k| k.index == i && k.page == keys.rapid_page).map(|k| k.lamp)
                    }
                    Lamp::Message => Some(keys.message_lamp),
                    Lamp::Mute => Some(keys.mute_lamp),
                    Lamp::Headset => Some(keys.headset_lamp),
                    Lamp::Speaker => Some(keys.speaker_lamp),
                },
            };
            self.blinker.set(lamp, state.unwrap_or(LampState::OFF));
        }
        let _ = self.blinker.tick(clock::mono_ms(), self.lamps.as_mut());
    }

    // ------------------------------------------------------------- draw

    fn draw(&mut self) {
        let begun = std::time::Instant::now();
        let st = self.store.snapshot();
        let cfg = self.cfg.clone();
        let store = self.store.clone();
        let (now_ms, unix) = self.now();
        let mut th = self.th;
        let idle_view = self.idle_view.clone();
        if let Some(v) = &idle_view {
            v.take_dirty();
        }
        let mut cx = Cx {
            store: &store,
            st: &st,
            th,
            model: self.model,
            cfg: &cfg,
            now_ms,
            unix,
            off_hook: self.off_hook,
            idle_view: idle_view.as_ref(),
        };
        let c = &mut self.main;
        let bounds = c.bounds();
        let top = self.nav.stack.last_mut().unwrap_or(&mut self.nav.base);
        // The configured wallpaper and palette, unless the screen is shown
        // with a wallpaper of its own (the boot screen, and the theme
        // selector, which shows the one its selection is on).
        let mut backdrop = match &cfg.wallpaper_picture {
            Some(picture) => Backdrop::Picture(picture),
            None => Backdrop::Builtin(wallpaper::find_or_default(&cfg.theme)),
        };
        if !th.gray
            && let Some(own) = top.look(&cx)
        {
            backdrop = Backdrop::Builtin(own);
            th = Theme::for_model(self.model).in_wallpaper(own);
            cx.th = th;
        }
        // A wallpaper that moves does so on the idle screen, while the
        // backlight gives light: the frame of this time now, and the next
        // one when its time has come.
        let started = clock::mono_ms();
        let mut frame_ms = None;
        let motion_time = self.motion_time.or(self.fixed_time.is_none().then_some(now_ms));
        if let (Backdrop::Builtin(wallpaper), Some(t)) = (backdrop, motion_time)
            && !th.gray
            // (The touch skin's wallpapers do not move yet.)
            && !th.touch
            && self.backlight_on
            && top.motion()
            && top.wallpaper()
            && !top.full_screen()
            && let Some(every) = wallpaper.frame_ms()
        {
            backdrop = Backdrop::Moving(wallpaper, t);
            frame_ms = Some(u64::from(every));
        }
        // Over a wallpaper in motion the screen is drawn three times: over
        // black and over white, which together tell what the screen puts
        // over the wallpaper ([`crate::gfx::Overlay`]), and then as it is
        // shown.  The frames that follow are made from the wallpaper and
        // that ([`Ui::motion_frame`]) until something else changes.
        let passes: &[Pass] = if frame_ms.is_some() { &[Pass::OverBlack, Pass::OverWhite, Pass::Shown] } else { &[Pass::Shown] };
        let mut serial = None;
        for pass in passes {
            // The regions to touch are those of this picture.
            c.clear_hits();
            if top.full_screen() {
                top.draw(&cx, c, bounds);
            } else {
                // The whole frame gets a background first: the softkey shapes
                // and the line-key tabs sit on it.
                match pass {
                    Pass::OverBlack => c.clear(0x000000),
                    Pass::OverWhite => c.clear(0xFFFFFF),
                    Pass::Shown if top.wallpaper() => serial = c.wallpaper(th.bg, backdrop),
                    Pass::Shown => c.clear(th.bg),
                }
                if top.softkey_bar(&cx) && th.touch {
                    // Buttons: an app's four softkeys are the four keys
                    // (and its window keeps its size, whatever they are);
                    // a native screen's are its actions, in as many rows
                    // as they take, and in none when it has none.
                    let (keys, rows): (Vec<(Hit, String)>, i32) = match top.raw_softkeys(&cx) {
                        Some(labels) => (
                            labels.into_iter().enumerate().map(|(i, l)| (Hit::Key(Key::Soft(i as u8 + 1)), l)).collect(),
                            1,
                        ),
                        None => {
                            let keys = touch_soft_slots(&top.softkeys(&cx), self.nav.soft_page);
                            let rows = if keys.iter().all(|(_, l)| l.is_empty()) { 0 } else { th.soft_rows(keys.len()) };
                            (keys.into_iter().map(|(id, l)| (Hit::Soft(id), l)).collect(), rows)
                        }
                    };
                    let (area, _) = bounds.split_bottom(rows * th.softkey_h);
                    top.draw(&cx, c, area);
                    widgets::soft_buttons(c, th, &keys);
                } else if top.softkey_bar(&cx) {
                    let (area, _) = bounds.split_bottom(th.softkey_h);
                    top.draw(&cx, c, area);
                    let labels = match top.raw_softkeys(&cx) {
                        Some(l) => l,
                        None => soft_slots(&top.softkeys(&cx), self.nav.soft_page).map(|(_, l)| l),
                    };
                    widgets::softkey_bar(c, th, &labels);
                } else {
                    top.draw(&cx, c, bounds);
                }
            }

            // Volume popup.
            if self.popup.is_some() {
                let a = &st.audio;
                // The ringer sounds on the speaker (for a call, or to let the
                // user hear the volume the keys just set): that is the
                // ringer's volume, not the speaker's.
                let (label, icon) = match a.current_target() {
                    VolumeTarget::Handset => (tr::VOL_HANDSET, "handset"),
                    VolumeTarget::Headset => (tr::VOL_HEADSET, "headset"),
                    VolumeTarget::Speaker => (tr::VOL_SPEAKER, "speaker"),
                    VolumeTarget::Ringer | VolumeTarget::Current => (tr::VOL_RINGER, "ring"),
                };
                let (w, h) = if th.gray { (bounds.w * 3 / 4, th.line_h * 2 + 12) } else { (th.px(300), th.px(84)) };
                // Touch: a touch beside the popup takes it down, and one
                // on it does nothing.
                if th.touch {
                    c.hit(bounds, Hit::Dismiss);
                }
                let inner = widgets::modal(c, th, bounds, w, h);
                if th.touch {
                    c.hit(inner.inset(-th.pad), Hit::Nothing);
                }
                let (head, rest) = inner.split_top(th.line_h);
                let (ic, text) = head.split_left(th.icon + th.pad);
                if let Some(i) = th.icon(icon) {
                    c.icon_in(i, IRect { w: th.icon, ..ic }, th.fg);
                }
                c.text_in(th.font(crate::appview::Font::Bold), text, label, th.fg, HAlign::Left);
                let mh = if th.gray { 8 } else { th.px(18) };
                // Touch: the volume keys, at the two ends of the meter.
                let rest = if th.touch {
                    let (minus, rest) = rest.split_left(th.touch_min);
                    let (rest, plus) = rest.split_right(th.touch_min);
                    for (r, label, key) in [(minus, "\u{2013}", Key::VolumeDown), (plus, "+", Key::VolumeUp)] {
                        widgets::button(c, th, r, label, th.sk_bg, Some(th.sk_edge), th.sk_fg, Hit::Key(key));
                    }
                    rest.inset_xy(th.pad, 0)
                } else {
                    rest
                };
                widgets::meter(
                    c,
                    th,
                    IRect::new(rest.x, rest.y + (rest.h - mh) / 2, rest.w, mh),
                    a.volume(VolumeTarget::Current) as u32,
                    ds::VOLUME_MAX as u32,
                );
            }

            // The newest notice.
            if let Some(n) = st.ui.notices.last() {
                let f = th.font(crate::appview::Font::Normal);
                let w = if th.gray { bounds.w - 12 } else { th.px(380) };
                let lines = f.wrap(&n.text, w - 2 * th.pad).len().clamp(1, 4) as i32;
                let h = (lines + 1) * th.line_h + 2 * th.pad + 2;
                // Touch: a touch anywhere takes the notice down, as Select,
                // Cancel, and the softkeys do.
                if th.touch {
                    c.hit(bounds, Hit::Dismiss);
                }
                let inner = widgets::modal(c, th, bounds, w, h);
                let (head, rest) = inner.split_top(th.line_h);
                let (icon, tint) = match n.level {
                    ds::NoticeLevel::Info => ("info", th.accent),
                    ds::NoticeLevel::Warning => ("warning", th.warn),
                    ds::NoticeLevel::Error => ("error", th.error),
                };
                let (ic, text) = head.split_left(th.icon + th.pad);
                if let Some(i) = th.icon(icon) {
                    c.icon_in(i, IRect { w: th.icon, ..ic }, tint);
                }
                c.text_in(th.font(crate::appview::Font::Bold), text, &n.title, th.fg, HAlign::Left);
                c.text_wrapped(f, rest, &n.text, th.fg, HAlign::Left, th.line_h);
            }
            match pass {
                Pass::OverBlack => self.over.over_black(c),
                Pass::OverWhite => self.over.over_white(c),
                Pass::Shown => {}
            }
        }
        self.screen_deadline = top.deadline(&cx);
        let name = top.name();
        let dial = top.dial_buffer().unwrap_or("").to_string();
        self.moving = match (backdrop, frame_ms, serial) {
            (Backdrop::Moving(wallpaper, _), Some(every), Some(serial)) => Some(Moving { wallpaper, every, serial }),
            _ => None,
        };

        if let Err(e) = c.present(self.display.as_mut(), opendeskphone_hal::Screen::Main) {
            log::warn!("display: {e}");
        }
        if let Some(sc) = &mut self.side {
            side::draw(&cx, sc);
            if let Err(e) = sc.present(self.display.as_mut(), opendeskphone_hal::Screen::Side) {
                log::warn!("side display: {e}");
            }
        }
        if !self.lit {
            self.light_up();
        }
        // The next frame is due one frame's time after this one began,
        // and never sooner than half of that after it was done, however
        // long drawing takes: the phone has other things to do.
        self.motion_deadline = frame_ms.map(|every| {
            let done = clock::mono_ms();
            (started + every).max(done + every / 2)
        });
        self.motion_due = false;
        if let Some(moving) = self.moving {
            self.motion_cost.frame(moving.wallpaper.id, moving.every, started, None, begun.elapsed());
        }
        self.store.update::<ds::Ui, _>(|u| {
            if u.screen != name {
                u.screen = name;
            }
            if u.dial_buffer != dial {
                u.dial_buffer = dial;
            }
        });
        self.needs_draw = false;
    }

    /// The next frame of the wallpaper in motion, when nothing else on
    /// the screen has changed since it was drawn: the pixels the motion
    /// changed, with what the screen draws over them, go to the display.
    /// The screen is not drawn again.
    fn motion_frame(&mut self) {
        self.motion_due = false;
        let (now_ms, _) = self.now();
        let (Some(moving), Some(t)) = (self.moving, self.motion_time.or(self.fixed_time.is_none().then_some(now_ms))) else {
            return self.draw();
        };
        let started = clock::mono_ms();
        let begun = std::time::Instant::now();
        let (c, over, display) = (&mut self.main, &self.over, self.display.as_mut());
        let mut drawn = Duration::ZERO;
        let serial = moving.wallpaper.with_frame_and_changes(t, |frame, changes| {
            drawn = begun.elapsed();
            // Another user of the wallpaper may have drawn a frame since
            // (the changes are then from a frame this screen never had).
            if changes.before != moving.serial {
                return None;
            }
            match c.patch(frame, changes.runs, over, display, opendeskphone_hal::Screen::Main) {
                Ok(true) => Some(changes.serial),
                Ok(false) => None,
                Err(e) => {
                    log::warn!("display: {e}");
                    None
                }
            }
        });
        let Some(serial) = serial else { return self.draw() };
        self.moving = Some(Moving { serial, ..moving });
        self.motion_frames += 1;
        let done = clock::mono_ms();
        self.motion_deadline = Some((started + moving.every).max(done + moving.every / 2));
        self.motion_cost.frame(moving.wallpaper.id, moving.every, started, Some(drawn), begun.elapsed());
    }

    // ------------------------------------------------------------- loop

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Input(ev) => self.input_event(ev, InputSource::Physical),
            Msg::State => {
                self.state_pending.store(false, Ordering::Release);
                self.state_changed();
            }
            Msg::Command(env) => self.command(env),
            Msg::OpenApp(view) => {
                view.set_theme(self.th);
                let tx = self.tx.clone();
                view.set_ui_waker(Some(Box::new(move || {
                    let _ = tx.send(Msg::Wake);
                })));
                self.activity();
                self.with_cx(|nav, cx| nav.open_app(cx, view));
            }
            Msg::IdleView(view) => {
                view.set_theme(self.th);
                self.drop_idle_view(view.name().to_string().as_str());
                let tx = self.tx.clone();
                view.set_ui_waker(Some(Box::new(move || {
                    let _ = tx.send(Msg::Wake);
                })));
                self.idle_views.push(view);
            }
            Msg::NoIdleView(name) => self.drop_idle_view(&name),
            Msg::CloseApp(name) => self.with_cx(|nav, cx| nav.close_app(cx, &name)),
            Msg::Config(mut cfg) => {
                cfg.contrast_max = self.cfg.contrast_max;
                let brightness_changed = cfg.brightness != self.cfg.brightness;
                let dimming_changed = cfg.dim_backlight != self.cfg.dim_backlight
                    || cfg.backlight_dim_level != self.cfg.backlight_dim_level;
                let contrast_changed = cfg.contrast != self.cfg.contrast;
                // A new theme is a new palette, on every screen, now.
                let th = configured_theme(self.model, &cfg);
                if !std::ptr::eq(th, self.th) {
                    self.th = th;
                    *self.theme_cell.lock().unwrap_or_else(|p| p.into_inner()) = th;
                    for screen in std::iter::once(&mut self.nav.base).chain(&mut self.nav.stack) {
                        screen.theme_changed(th);
                    }
                    for view in self.idle_views.iter().chain(&self.idle_view) {
                        view.set_theme(th);
                    }
                }
                self.cfg = Arc::new(cfg);
                if brightness_changed {
                    self.level = 0;
                    self.apply_light(self.light);
                } else if dimming_changed {
                    // Dimming switched off lights a dimmed backlight again.
                    self.apply_light(if self.cfg.dim_backlight { self.light } else { Light::On });
                }
                if contrast_changed && self.cfg.contrast_max > 0 {
                    let c = self.cfg.contrast.min(self.cfg.contrast_max);
                    let _ = self.backlight.set_contrast(opendeskphone_hal::Screen::Main, c);
                    self.store.update::<ds::Ui, _>(|u| u.contrast = c);
                }
                self.needs_draw = true;
            }
            Msg::Wake => self.needs_draw = true,
            Msg::Capture => {
                for screen in self.display.screens() {
                    let _ = self.display.capture(screen);
                }
            }
            Msg::Quit => self.quit = true,
        }
    }

    fn state_changed(&mut self) {
        // A failed update becomes a notice (the update screen goes away).
        let failed = self.store.read::<ds::Update, _>(|u| match &u.phase {
            UpdatePhase::Failed(e) => Some(e.clone()),
            _ => None,
        });
        match (failed, self.last_failed_update) {
            (Some(e), false) => {
                self.last_failed_update = true;
                self.notify(ds::NoticeLevel::Error, tr::UPD_FAILED.into(), e, 0);
            }
            (None, true) => self.last_failed_update = false,
            _ => {}
        }
        let ringing = self.store.read::<ds::Calls, _>(|c| c.ringing().next().is_some());
        // An alarm that begins to ring lights the screen, as a call does.
        let alarm = self.store.read::<ds::Alarms, _>(|a| a.ringing.as_ref().map(|r| r.id));
        if ringing || (alarm.is_some() && alarm != self.nav.alarm_shown) {
            self.activity();
        }
        self.with_cx(|nav, cx| nav.sync(cx));
    }

    /// Deadline-driven work: screen ticks, popups, notices, lamps, light.
    fn timers(&mut self) {
        let (now_ms, _) = self.now();
        let real_now = now_ms;
        let real_now_mono = clock::mono_ms();
        if self.popup.as_ref().is_some_and(|p| real_now >= p.until_ms) {
            self.popup = None;
            self.needs_draw = true;
        }
        let expired: Vec<u64> = self.notice_deadlines.iter().filter(|(_, d)| real_now >= *d).map(|(i, _)| *i).collect();
        for id in expired {
            self.dismiss_notice(id);
        }
        // The deadline the top screen gave when it was last drawn.
        let due = self.screen_deadline.is_some_and(|d| now_ms >= d);
        if due {
            self.screen_deadline = None;
            self.with_cx(|nav, cx| {
                let r = nav.top().tick(cx);
                nav.apply(cx, r);
            });
        }
        // The trial cover has had its time.
        if self.nav.kind == BaseKind::Finishing && self.nav.cover_until.is_some_and(|t| now_ms >= t) {
            self.with_cx(|nav, cx| nav.sync(cx));
        }
        if self.nav.top_ref().dirty() {
            self.needs_draw = true;
        }
        if self.motion_deadline.is_some_and(|d| real_now_mono >= d) {
            self.motion_due = true;
        }
        // A finger that has stayed where it came down: a long press.
        if let Some(g) = self.finger.tick(now_ms) {
            self.gesture(g, InputSource::Physical);
        }
        self.check_light();
    }

    fn next_deadline(&self) -> Option<u64> {
        let now = clock::mono_ms();
        let mut d: Option<u64> = None;
        let mut add = |t: Option<u64>| {
            if let Some(t) = t {
                d = Some(d.map_or(t, |x: u64| x.min(t)));
            }
        };
        add(self.popup.as_ref().map(|p| p.until_ms));
        add(self.notice_deadlines.iter().map(|(_, t)| *t).min());
        add(self.light_deadline());
        add(self.motion_deadline);
        if self.idle_views.len() > 1 {
            add(Some(self.idle_turn_ms));
        }
        if self.blinker.needs_ticks() {
            add(Some(now + BLINK_TICK_MS));
        }
        if self.fixed_time.is_none() {
            add(self.finger.deadline());
            add(self.screen_deadline);
            if self.nav.kind == BaseKind::Finishing {
                add(self.nav.cover_until);
            }
        }
        d
    }

    /// Processes everything that is pending without blocking, then
    /// repaints if needed.  [`Ui::run`] is this in a loop; tests and the
    /// snapshot harness call it directly.  While the UI still owns the
    /// input device (before `run`), it is polled here too.
    pub fn pump(&mut self) {
        loop {
            let mut any = false;
            loop {
                let ev = match &mut self.input {
                    Some(input) => input.next_event(Some(Duration::ZERO)).ok().flatten(),
                    None => None,
                };
                let Some(ev) = ev else { break };
                self.input_event(ev, InputSource::Physical);
                any = true;
            }
            while let Some(env) = self.commands.as_ref().and_then(|rx| rx.try_recv()) {
                self.command(env);
                any = true;
            }
            while let Ok(msg) = self.rx.try_recv() {
                self.on_msg(msg);
                any = true;
            }
            if !any {
                break;
            }
        }
        self.finish_turn();
    }

    fn drop_idle_view(&mut self, name: &str) {
        self.idle_views.retain(|v| {
            if v.name() == name {
                v.set_ui_waker(None);
            }
            v.name() != name
        });
        if self.idle_view.as_ref().is_some_and(|v| v.name() == name) {
            self.idle_view = None;
            self.idle_shown = false;
            self.needs_draw = true;
        }
    }

    /// Chooses which app's idle view has the idle screen's app area and
    /// tells the views when they come on screen and leave it.  The idle
    /// screen shows them while it is the top screen (so not during a
    /// call); among the views that have a widget, the least recently shown
    /// takes the next turn every `idle_cycle_secs`.
    fn sync_idle_view(&mut self) {
        let now = clock::mono_ms();
        let cycle = self.cfg.idle_cycle_secs as u64 * 1000;
        let has_widgets = |v: &AppView| !v.batch_quiet().roots.is_empty();
        let due = now >= self.idle_turn_ms;
        let current_ok = cycle > 0 && self.idle_view.as_ref().is_some_and(has_widgets);
        if !current_ok || due {
            // The current view goes to the back of the line; the first
            // view with something to show is next (it may be the same one).
            let next = if cycle == 0 { None } else { self.idle_views.iter().position(has_widgets) };
            let next = next.map(|i| self.idle_views.remove(i));
            if let Some(v) = &next {
                self.idle_views.push(v.clone());
            }
            let changed = match (&self.idle_view, &next) {
                (Some(a), Some(b)) => a.name() != b.name(),
                (None, None) => false,
                _ => true,
            };
            if changed {
                if let Some(old) = &self.idle_view
                    && std::mem::take(&mut self.idle_shown)
                {
                    old.push_event(AppEvent::Background);
                }
                self.idle_view = next;
                self.needs_draw = true;
            }
            self.idle_turn_ms = now + cycle.max(1000);
        }
        let Some(view) = &self.idle_view else { return };
        let shown = self.nav.kind == BaseKind::Idle && self.nav.stack.is_empty();
        if shown != self.idle_shown {
            self.idle_shown = shown;
            view.push_event(if shown { AppEvent::Foreground } else { AppEvent::Background });
        }
    }

    fn finish_turn(&mut self) {
        self.timers();
        self.sync_idle_view();
        self.sync_lamps();
        if self.needs_draw {
            self.draw();
        } else if self.motion_due {
            self.motion_frame();
        }
        self.store.heartbeat(Subsystem::Ui);
    }

    /// Runs the UI until [`UiHandle::quit`].  Spawns two small helper
    /// threads: one blocks on the input device, one on the command queue.
    pub fn run(mut self) {
        if let Some(mut input) = self.input.take() {
            let tx = self.tx.clone();
            let _ = std::thread::Builder::new().name("ui-input".into()).stack_size(64 * 1024).spawn(move || loop {
                match input.next_event(None) {
                    Ok(Some(ev)) => {
                        if tx.send(Msg::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::error!("input: {e}");
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
            });
        }
        if let Some(commands) = self.commands.take() {
            let tx = self.tx.clone();
            let _ = std::thread::Builder::new().name("ui-commands".into()).stack_size(64 * 1024).spawn(move || {
                while let Some(env) = commands.recv() {
                    if tx.send(Msg::Command(env)).is_err() {
                        break;
                    }
                }
            });
        }
        self.state_changed();
        self.finish_turn();
        while !self.quit {
            let now = clock::mono_ms();
            // Wake at least every 10 s for the heartbeat.
            let wait = self.next_deadline().map_or(10_000, |d| d.saturating_sub(now).min(10_000));
            match self.rx.recv_timeout(Duration::from_millis(wait.max(1))) {
                Ok(msg) => {
                    self.on_msg(msg);
                    while let Ok(msg) = self.rx.try_recv() {
                        self.on_msg(msg);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.finish_turn();
        }
        self.store.set_health(Subsystem::Ui, ds::HealthStatus::Stopped);
    }

    /// The main canvas (tests and tools).
    pub fn canvas(&self) -> &Canvas {
        &self.main
    }
    /// The D70 side canvas.
    pub fn side_canvas(&self) -> Option<&Canvas> {
        self.side.as_ref()
    }
    /// Name of the top screen (`idle`, `call`, `app:voicemail`, ...).
    pub fn screen_name(&self) -> String {
        self.nav.top_ref().name()
    }
}
