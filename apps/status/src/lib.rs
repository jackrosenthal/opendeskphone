//! Status: choose the user's presence from the options the PBX offers and
//! set an optional message.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{InputMode, PbxPresenceState, PresenceStatus};

const SK_SELECT: u32 = 1;
const SK_MESSAGE: u32 = 2;
const SK_EXIT: u32 = 3;
const SK_OK: u32 = 4;
const SK_BACK: u32 = 5;
const SK_RETRY: u32 = 6;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out, and a touch selects a row.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_BACK, SK_SELECT])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}

#[derive(Clone)]
struct Choice {
    id: u32,
    state: PbxPresenceState,
    sub: String,
    active: bool,
}

impl Choice {
    fn label(&self) -> String {
        let base = presence_label(self.state);
        if self.sub.is_empty() {
            base.to_string()
        } else {
            format!("{base} - {}", self.sub)
        }
    }
}

fn presence_label(state: PbxPresenceState) -> &'static str {
    match state {
        PbxPresenceState::Available => "Available",
        PbxPresenceState::Chat => "Chat",
        PbxPresenceState::Away => "Away",
        PbxPresenceState::Dnd => "Do Not Disturb",
        PbxPresenceState::Xa => "Extended Away",
        PbxPresenceState::Unavailable => "Unavailable",
        PbxPresenceState::NotSet => "Not Set",
    }
}

/// The phone's presence for a state of the PBX's.
fn phone_presence(state: PbxPresenceState) -> PresenceStatus {
    match state {
        PbxPresenceState::Available => PresenceStatus::Available,
        PbxPresenceState::Chat => PresenceStatus::Chat,
        PbxPresenceState::Away => PresenceStatus::Away,
        PbxPresenceState::Dnd => PresenceStatus::Dnd,
        PbxPresenceState::Xa => PresenceStatus::Xa,
        PbxPresenceState::Unavailable => PresenceStatus::Unavailable,
        PbxPresenceState::NotSet => PresenceStatus::Unknown,
    }
}

enum Screen {
    Loading,
    List { list: Widget },
    Message { form: Form },
    Error,
}

struct Status {
    screen: Screen,
    options: Vec<Choice>,
    message: String,
    list_req: Option<u32>,
    info_req: Option<u32>,
    update_req: Option<u32>,
    /// Option being applied, for the phone's own presence on success.
    applying: Option<Choice>,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            screen: Screen::Loading,
            options: Vec::new(),
            message: String::new(),
            list_req: None,
            info_req: None,
            update_req: None,
            applying: None,
        }
    }
}

impl Status {
    fn load(&mut self) {
        Message::show("Status", "Loading...", &keys(&[Softkey::new(SK_EXIT, "Exit")]));
        self.screen = Screen::Loading;
        self.list_req = Some(pbx::presence_options());
        self.info_req = Some(pbx::presence_info());
    }

    fn show_list(&mut self) {
        let r = screen("Status");
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        // The status that is set: "*" at the right; touch: a check mark
        // at the left, and no highlighted row.
        let items: Vec<ListItem> = self
            .options
            .iter()
            .map(|o| match has_keys() {
                true => ListItem::with_detail(o.label(), if o.active { "*" } else { "" }),
                false => ListItem {
                    text: o.label(),
                    detail: String::new(),
                    icon: o.active.then_some(Image::Icon(Icon::Ok)),
                },
            })
            .collect();
        ui::set_items(list, &items);
        if let Some(i) = self.options.iter().position(|o| o.active) {
            ui::set_selected(list, i as u32);
        }
        set_keys(&[
            Softkey::new(SK_SELECT, "Select"),
            Softkey::new(SK_MESSAGE, "Message"),
            Softkey::new(SK_EXIT, "Exit"),
        ]);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.screen = Screen::List { list };
    }

    fn show_error(&mut self, text: &str) {
        Message::show(
            "Status",
            text,
            &keys(&[
                Softkey::new(SK_RETRY, "Retry"),
                Softkey::new(SK_EXIT, "Exit"),
            ]),
        );
        self.screen = Screen::Error;
    }

    fn apply(&mut self, index: usize) {
        let Some(choice) = self.options.get(index).cloned() else {
            return;
        };
        self.update_req = Some(pbx::set_presence(choice.id, &self.message));
        self.applying = Some(choice);
    }

    fn active_index(&self) -> Option<usize> {
        self.options.iter().position(|o| o.active)
    }

    fn on_pbx(&mut self, id: u32, outcome: Result<PbxResult, PbxError>) {
        if self.list_req == Some(id) {
            self.list_req = None;
            match outcome {
                Ok(PbxResult::PresenceOptions(options)) => {
                    self.options = options
                        .into_iter()
                        .map(|o| Choice {
                            id: o.id,
                            state: o.state,
                            sub: o.sub_state,
                            active: o.active,
                        })
                        .collect();
                    if self.options.is_empty() {
                        self.show_error("No status options are available.");
                    } else {
                        self.show_list();
                    }
                }
                Ok(_) => self.show_error("No status options are available."),
                Err(e) => self.show_error(&e.to_string()),
            }
        } else if self.info_req == Some(id) {
            self.info_req = None;
            if let Ok(PbxResult::Presence(p)) = outcome {
                self.message = p.message;
            }
        } else if self.update_req == Some(id) {
            self.update_req = None;
            match outcome {
                Ok(_) => {
                    if let Some(c) = self.applying.take() {
                        let _ = presence::set_status(phone_presence(c.state), &self.message);
                        for o in &mut self.options {
                            o.active = o.id == c.id;
                        }
                    }
                    runtime::background();
                }
                Err(e) => {
                    self.applying = None;
                    self.show_error(&e.to_string());
                }
            }
        }
    }
}

impl App for Status {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => self.load(),
            Event::Pbx((id, outcome)) => self.on_pbx(id, outcome),
            Event::Widget((_, WidgetEvent::Activated(i))) => {
                if matches!(self.screen, Screen::List { .. }) {
                    self.apply(i as usize);
                }
            }
            Event::Widget((_, WidgetEvent::Submitted(text))) => {
                if matches!(self.screen, Screen::Message { .. }) {
                    self.message = text;
                    match self.active_index() {
                        Some(i) => self.apply(i),
                        None => self.show_list(),
                    }
                }
            }
            Event::Softkey(SK_SELECT) => {
                if let Screen::List { list } = self.screen
                    && let Some(row) = ui::selected(list)
                {
                    self.apply(row as usize);
                }
            }
            Event::Softkey(SK_MESSAGE) => {
                let form = Form::show(
                    "Status Message",
                    &[("Message", InputMode::Text, &self.message)],
                    &keys(&[Softkey::new(SK_OK, "OK"), Softkey::new(SK_BACK, "Back")]),
                );
                self.screen = Screen::Message { form };
            }
            Event::Softkey(SK_OK) => {
                if let Screen::Message { form } = &self.screen {
                    self.message = form.value(0);
                    match self.active_index() {
                        Some(i) => self.apply(i),
                        None => self.show_list(),
                    }
                }
            }
            Event::Softkey(SK_BACK) => self.show_list(),
            Event::Softkey(SK_RETRY) => self.load(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => match self.screen {
                Screen::Message { .. } => self.show_list(),
                _ => runtime::background(),
            },
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Status,
    name: "status",
    id: "org.opendeskphone.status",
    display_name: "Status",
    description: "Set your presence status",
    phone_icon: Some(Icon::AppStatus),
}
