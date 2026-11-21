//! Queues: the user's call queues with their login state; log in and out,
//! pause and resume, and view a queue's numbers for today.
//!
//! On the touchscreen, which has no keys, touching a queue opens its
//! screen: where the user stands in it, in a sentence, and today's
//! figures, a line each, with the buttons that log in and out, pause and
//! resume, and refresh across the bottom.  The phone's back button goes
//! back to the list.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::controls::{self, Button, GAP, Look};
use opendeskphone_app_sdk::{Align, InputMode, QueueMembership, QueueStatus, nav};

const SK_LOGIN: u32 = 1;
const SK_LOGOUT: u32 = 2;
const SK_PAUSE: u32 = 3;
const SK_RESUME: u32 = 4;
const SK_DETAILS: u32 = 5;
const SK_EXIT: u32 = 6;
const SK_BACK: u32 = 7;
const SK_OK: u32 = 8;
const SK_REFRESH: u32 = 9;
const REFRESH_MS: u32 = 15_000;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    nav::softkeys(keys, &[SK_EXIT, SK_BACK])
}

#[derive(Clone)]
struct Queue {
    id: String,
    name: String,
    status: QueueMembership,
    paused: bool,
}

/// What to ask of the PBX about the selected queue.
enum Action {
    Login,
    Logout,
    Pause(String),
    Resume,
}

impl Queue {
    fn state_text(&self) -> &str {
        use QueueMembership::*;
        match (self.status, self.paused) {
            (LoggedIn | Permanent, true) => "Paused",
            (LoggedIn, false) => "In",
            (Permanent, false) => "Always",
            (Manager, _) => "Manager",
            (LoggedOut, _) => "Out",
        }
    }
    fn member(&self) -> bool {
        matches!(self.status, QueueMembership::LoggedIn | QueueMembership::Permanent)
    }
    /// Touch: where the user stands in the queue, in a sentence.
    fn sentence(&self) -> &str {
        use QueueMembership::*;
        match (self.status, self.paused) {
            (LoggedIn | Permanent, true) => "You are paused in this queue.",
            (LoggedIn, false) => "You are logged in to this queue.",
            (Permanent, false) => "You are always in this queue.",
            (Manager, _) => "You manage this queue.",
            (LoggedOut, _) => "You are logged out of this queue.",
        }
    }
}

#[derive(Default)]
enum Screen {
    #[default]
    Loading,
    List,
    Details,
    Reason(Form),
    /// The user has no queues.
    Empty,
    Error,
}

#[derive(Default)]
struct Queues {
    queues: Vec<Queue>,
    screen: Screen,
    selected: usize,
    list_req: Option<u32>,
    action_req: Option<u32>,
    stats_req: Option<u32>,
    timer: Option<u32>,
    shown: bool,
    /// Touch: the buttons of a queue's screen, each with the softkey whose
    /// work it does.
    buttons: Vec<(Button, u32)>,
}

impl Queues {
    fn refresh(&mut self) {
        self.list_req = Some(pbx::queues());
    }

    fn current(&self) -> Option<&Queue> {
        self.queues.get(self.selected)
    }

    /// The softkeys that change the user's state in the queue.
    fn state_keys(&self) -> Vec<Softkey> {
        let mut keys = Vec::new();
        if let Some(q) = self.current() {
            match q.status {
                QueueMembership::LoggedIn => keys.push(Softkey::new(SK_LOGOUT, "Log Out")),
                QueueMembership::LoggedOut => keys.push(Softkey::new(SK_LOGIN, "Log In")),
                _ => {}
            }
            if q.member() {
                keys.push(if q.paused {
                    Softkey::new(SK_RESUME, "Resume")
                } else {
                    Softkey::new(SK_PAUSE, "Pause")
                });
            }
        }
        keys
    }

    /// The list's softkeys: for the highlighted queue.  Touch: none (no
    /// queue is the highlighted one; a queue's softkeys are with its
    /// numbers).
    fn softkeys(&self) -> Vec<Softkey> {
        if !has_keys() {
            return Vec::new();
        }
        let mut keys = self.state_keys();
        if self.current().is_some() {
            keys.push(Softkey::new(SK_DETAILS, "Details"));
        }
        keys.push(Softkey::new(SK_EXIT, "Exit"));
        keys
    }

    /// The softkeys of a queue's numbers, loaded or not.  (Touch: the
    /// queue's screen has buttons instead.)
    fn detail_keys(&self, loaded: bool) -> Vec<Softkey> {
        let mut keys = Vec::new();
        if loaded {
            keys.push(Softkey::new(SK_REFRESH, "Refresh"));
        }
        keys.push(Softkey::new(SK_BACK, "Back"));
        keys
    }

    fn draw_list(&mut self) {
        let r = screen("Queues");
        if self.queues.is_empty() {
            let text = if has_keys() {
                "You are not a member of any queue."
            } else {
                "You are not a member of any queue.  Your administrator can add you to one."
            };
            opendeskphone_app_sdk::nav::note(r, text);
            ui::set_softkeys(ROOT, &keys(&[Softkey::new(SK_EXIT, "Exit")]));
            self.screen = Screen::Empty;
            return;
        }
        self.selected = self.selected.min(self.queues.len() - 1);
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = self
            .queues
            .iter()
            .map(|q| ListItem::with_detail(q.name.clone(), q.state_text()))
            .collect();
        ui::set_items(list, &items);
        ui::set_selected(list, self.selected as u32);
        ui::set_softkeys(ROOT, &self.softkeys());
        nav::focus_list(list);
        self.screen = Screen::List;
    }

    fn action(&mut self, action: Action) {
        let Some(q) = self.current() else { return };
        let queue = Some(q.id.as_str());
        self.action_req = Some(match action {
            Action::Login => pbx::queue_login(queue),
            Action::Logout => pbx::queue_logout(queue),
            Action::Pause(reason) => pbx::queue_pause(queue, &reason),
            Action::Resume => pbx::queue_resume(queue),
        });
    }

    fn show_details(&mut self) {
        let Some(q) = self.current().cloned() else {
            return;
        };
        if has_keys() {
            Message::show(&q.name, "Loading...", &self.detail_keys(false));
        } else {
            self.draw_details_touch(None);
        }
        self.screen = Screen::Details;
        self.stats_req = Some(pbx::queue_status(&q.id));
    }

    /// Touch: a queue's screen.  Under the title, where the user stands
    /// in the queue, and today's figures, a line each; across the bottom,
    /// the buttons that change where the user stands, and "Refresh".
    fn draw_details_touch(&mut self, s: Option<&QueueStatus>) {
        let Some(q) = self.current().cloned() else {
            return;
        };
        self.buttons.clear();
        let m = ui::get_metrics();
        let area = screen(&q.name);
        let (_, bottom) = controls::split_bottom(area);
        let line = m.line_height * 3 / 2;
        let (x, width) = (GAP as i16, m.width - 2 * GAP);
        let mut y = area.y + GAP as i16;
        let text = |at: Rect, align: Align, words: &str| {
            let w = ui::create(WidgetKind::Text, ROOT, at);
            ui::set_align(w, align);
            ui::set_text(w, words);
        };
        text(Rect::new(x, y, width, line), Align::Left, q.sentence());
        y += (line + GAP) as i16;
        match s {
            None => text(Rect::new(x, y, width, line), Align::Left, "Loading\u{2026}"),
            Some(s) => {
                let figures = [
                    ("Callers waiting", s.callers_waiting.to_string()),
                    ("Members logged in", format!("{} of {}", s.members_logged_in, s.members)),
                    ("Members on calls", s.members_on_call.to_string()),
                    ("Calls today", s.calls.to_string()),
                    ("Completed", s.completed_calls.to_string()),
                    ("Abandoned", s.abandoned_calls.to_string()),
                ];
                for (label, value) in figures {
                    text(Rect::new(x, y, width, line), Align::Left, label);
                    text(Rect::new(x, y, width, line), Align::Right, &value);
                    y += line as i16;
                }
            }
        }
        let mut acts = Vec::new();
        if s.is_some() {
            acts.push(("Refresh", Look::Secondary, SK_REFRESH));
        }
        match (q.status, q.paused) {
            (QueueMembership::LoggedOut, _) => acts.push(("Log in", Look::Primary, SK_LOGIN)),
            (QueueMembership::LoggedIn | QueueMembership::Permanent, true) => {
                acts.push(("Resume", Look::Primary, SK_RESUME))
            }
            (QueueMembership::LoggedIn | QueueMembership::Permanent, false) => {
                acts.push(("Pause", Look::Secondary, SK_PAUSE))
            }
            _ => {}
        }
        if q.status == QueueMembership::LoggedIn {
            acts.push(("Log out", Look::Secondary, SK_LOGOUT));
        }
        let places = controls::columns(bottom, acts.len() as u16);
        for (place, (label, look, id)) in places.into_iter().zip(acts) {
            self.buttons.push((Button::new(ROOT, place, label, look), id));
        }
        ui::set_softkeys(ROOT, &[]);
    }

    fn draw_details(&mut self, s: &QueueStatus) {
        let Some(q) = self.current().cloned() else {
            return;
        };
        if !has_keys() {
            self.draw_details_touch(Some(s));
            self.screen = Screen::Details;
            return;
        }
        let text = format!(
            "Members: {} ({} logged in)\nOn calls: {}\nWaiting: {}\nCalls today: {}\nCompleted: {}\nAbandoned: {}",
            s.members,
            s.members_logged_in,
            s.members_on_call,
            s.callers_waiting,
            s.calls,
            s.completed_calls,
            s.abandoned_calls,
        );
        Message::show(&q.name, &text, &self.detail_keys(true));
        self.screen = Screen::Details;
    }

    fn error(&mut self, e: &PbxError) {
        Message::show(
            "Queues",
            &e.to_string(),
            &keys(&[Softkey::new(SK_BACK, "Back")]),
        );
        self.screen = Screen::Error;
    }

    /// Cancel: one screen back, or out of the app.
    fn back(&mut self) {
        match self.screen {
            Screen::Details => self.draw_list(),
            // Touch: the pause was asked for with the queue's numbers.
            Screen::Reason(_) if !has_keys() => self.show_details(),
            Screen::Reason(_) => self.draw_list(),
            // Touch: the back button is what the "Back" softkey is on
            // the other phones.
            Screen::Error if !has_keys() => self.draw_list(),
            _ => runtime::background(),
        }
    }

    fn on_pbx(&mut self, id: u32, outcome: Result<PbxResult, PbxError>) {
        if self.list_req == Some(id) {
            self.list_req = None;
            match outcome {
                Ok(result) => {
                    let queues = match result {
                        PbxResult::Queues(queues) => queues,
                        _ => Vec::new(),
                    };
                    self.queues = queues
                        .into_iter()
                        .map(|q| Queue {
                            id: q.id,
                            name: q.name,
                            status: q.membership,
                            paused: q.paused,
                        })
                        .collect();
                    // Do not pull the user out of a detail or input screen.
                    if matches!(self.screen, Screen::Loading | Screen::List | Screen::Empty | Screen::Error) {
                        self.draw_list();
                    }
                }
                Err(e) if matches!(self.screen, Screen::Loading) => self.error(&e),
                Err(_) => {}
            }
        } else if self.action_req == Some(id) {
            self.action_req = None;
            match outcome {
                Ok(_) => {
                    self.screen = Screen::Loading;
                    self.refresh();
                }
                Err(e) => self.error(&e),
            }
        } else if self.stats_req == Some(id) {
            self.stats_req = None;
            if matches!(self.screen, Screen::Details) {
                match outcome {
                    Ok(PbxResult::QueueStatus(s)) => self.draw_details(&s),
                    Ok(_) => self.error(&PbxError::Transport("the server's reply was not the method's".into())),
                    Err(e) => self.error(&e),
                }
            }
        }
    }
}

impl App for Queues {
    fn start(&mut self) {
        // Keep the queue list across visits, as the stock app does.
        runtime::set_exit_after_background(false);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                self.shown = true;
                if self.queues.is_empty() {
                    Message::show("Queues", "Loading...", &keys(&[Softkey::new(SK_EXIT, "Exit")]));
                    self.screen = Screen::Loading;
                } else {
                    self.draw_list();
                }
                self.refresh();
                self.timer = timers::set(REFRESH_MS, true).ok();
            }
            Event::Background | Event::Exit => {
                self.shown = false;
                if let Some(t) = self.timer.take() {
                    timers::cancel(t);
                }
            }
            Event::Timer(id) if self.timer == Some(id) => {
                if matches!(self.screen, Screen::List) {
                    self.refresh();
                }
            }
            Event::Pbx((id, outcome)) => self.on_pbx(id, outcome),
            Event::Widget((_, WidgetEvent::Highlighted(i))) if matches!(self.screen, Screen::List) => {
                self.selected = i as usize;
                ui::set_softkeys(ROOT, &self.softkeys());
            }
            Event::Widget((_, WidgetEvent::Activated(i))) if matches!(self.screen, Screen::List) => {
                self.selected = i as usize;
                self.show_details();
            }
            Event::Softkey(SK_LOGIN) => self.action(Action::Login),
            Event::Softkey(SK_LOGOUT) => self.action(Action::Logout),
            Event::Softkey(SK_RESUME) => self.action(Action::Resume),
            Event::Softkey(SK_PAUSE) => {
                let form = Form::show(
                    "Pause",
                    &[("Reason", InputMode::Text, "")],
                    &keys(&[Softkey::new(SK_OK, "Pause"), Softkey::new(SK_BACK, "Back")]),
                );
                self.screen = Screen::Reason(form);
            }
            Event::Softkey(SK_OK) | Event::Widget((_, WidgetEvent::Submitted(_))) => {
                if let Screen::Reason(form) = &self.screen {
                    let reason = form.value(0);
                    self.action(Action::Pause(reason));
                }
            }
            // Touch: a button of a queue's screen does what its softkey
            // does.
            Event::Touch((w, t)) => {
                let mut acted = None;
                for (b, id) in &mut self.buttons {
                    if b.touched(w, &t) {
                        acted = Some(*id);
                    }
                }
                if let Some(id) = acted {
                    self.handle(Event::Softkey(id));
                }
            }
            Event::Softkey(SK_DETAILS | SK_REFRESH) => self.show_details(),
            Event::Softkey(SK_BACK) => self.draw_list(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => self.back(),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Queues,
    name: "queues",
    id: "org.opendeskphone.queues",
    display_name: "Queues",
    description: "Log in to call queues and see how they are doing",
    phone_icon: Some(Icon::AppQueues),
}
