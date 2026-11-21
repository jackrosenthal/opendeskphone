//! Queues: the user's call queues with their login state; log in and out,
//! pause and resume, and view a queue's numbers for today.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{InputMode, QueueMembership, QueueStatus};

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
}

#[derive(Default)]
enum Screen {
    #[default]
    Loading,
    List,
    Details,
    Reason(Form),
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
}

impl Queues {
    fn refresh(&mut self) {
        self.list_req = Some(pbx::queues());
    }

    fn current(&self) -> Option<&Queue> {
        self.queues.get(self.selected)
    }

    fn softkeys(&self) -> Vec<Softkey> {
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
            keys.push(Softkey::new(SK_DETAILS, "Details"));
        }
        keys.push(Softkey::new(SK_EXIT, "Exit"));
        keys
    }

    fn draw_list(&mut self) {
        let r = screen("Queues");
        if self.queues.is_empty() {
            opendeskphone_app_sdk::nav::note(r, "You are not a member of any queue.");
            ui::set_softkeys(ROOT, &[Softkey::new(SK_EXIT, "Exit")]);
            self.screen = Screen::Error;
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
        ui::set_focus(list);
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
        Message::show(&q.name, "Loading...", &[Softkey::new(SK_BACK, "Back")]);
        self.screen = Screen::Details;
        self.stats_req = Some(pbx::queue_status(&q.id));
    }

    fn draw_details(&mut self, s: &QueueStatus) {
        let Some(q) = self.current().cloned() else {
            return;
        };
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
        Message::show(
            &q.name,
            &text,
            &[
                Softkey::new(SK_REFRESH, "Refresh"),
                Softkey::new(SK_BACK, "Back"),
            ],
        );
        self.screen = Screen::Details;
    }

    fn error(&mut self, e: &PbxError) {
        Message::show(
            "Queues",
            &e.to_string(),
            &[Softkey::new(SK_BACK, "Back")],
        );
        self.screen = Screen::Error;
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
                    if matches!(self.screen, Screen::Loading | Screen::List | Screen::Error) {
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
                    Message::show("Queues", "Loading...", &[Softkey::new(SK_EXIT, "Exit")]);
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
            Event::Widget((_, WidgetEvent::Highlighted(i))) => {
                self.selected = i as usize;
                ui::set_softkeys(ROOT, &self.softkeys());
            }
            // Touch: the touch that activates a queue is the one that
            // highlights it, and the softkeys are for the highlighted
            // one.  "Details" shows it.
            Event::Widget((_, WidgetEvent::Activated(_))) if !has_keys() => {}
            Event::Widget((_, WidgetEvent::Activated(i))) => {
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
                    &[Softkey::new(SK_OK, "Pause"), Softkey::new(SK_BACK, "Back")],
                );
                self.screen = Screen::Reason(form);
            }
            Event::Softkey(SK_OK) | Event::Widget((_, WidgetEvent::Submitted(_))) => {
                if let Screen::Reason(form) = &self.screen {
                    let reason = form.value(0);
                    self.action(Action::Pause(reason));
                }
            }
            Event::Softkey(SK_DETAILS | SK_REFRESH) => self.show_details(),
            Event::Softkey(SK_BACK) => self.draw_list(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => match self.screen {
                Screen::Details | Screen::Reason(_) => self.draw_list(),
                _ => runtime::background(),
            },
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
