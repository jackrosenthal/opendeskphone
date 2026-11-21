//! Visual voicemail: folders, the message list, playback, and message
//! actions, over the PBX's voicemail methods (`pbx::voicemail_*`).
//!
//! Playback is a call: `pbx::voicemail_play` makes the PBX ring this phone with
//! the event `digium.incomingCall.voicemail`; the app answers it while it
//! is on screen (and rejects it otherwise), and sends `*`, `#`, and `0` as
//! DTMF for rewind, skip, and pause.
//!
//! On the touchscreen, which has no keys, touching a message highlights
//! it, for the softkeys to act on ("Play" plays it), and rewind, pause,
//! and skip are softkeys while a message plays.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{VoicemailMessage, VoicemailSort};

const SK_PLAY: u32 = 1;
const SK_DELETE: u32 = 2;
const SK_FOLDERS: u32 = 3;
const SK_MARK: u32 = 4;
const SK_CALL: u32 = 5;
const SK_EXIT: u32 = 6;
const SK_BACK: u32 = 7;
const SK_YES: u32 = 8;
const SK_NO: u32 = 9;
const SK_STOP: u32 = 10;
const SK_OPEN: u32 = 11;
const SK_MOVE: u32 = 12;
const SK_REFRESH: u32 = 13;
const SK_REWIND: u32 = 14;
const SK_PAUSE: u32 = 15;
const SK_SKIP: u32 = 16;

const PLAYBACK_EVENT: &str = "digium.incomingCall.voicemail";
const PAGE: u32 = 30;

#[derive(Clone, Default)]
struct Msg {
    id: String,
    name: String,
    number: String,
    date: u64,
    duration: u32,
    read: bool,
}

impl From<VoicemailMessage> for Msg {
    fn from(m: VoicemailMessage) -> Msg {
        Msg {
            id: m.id,
            name: m.caller_name,
            number: m.caller_number,
            date: m.date,
            duration: m.duration,
            read: m.read,
        }
    }
}

impl Msg {
    fn who(&self) -> &str {
        if self.name.is_empty() {
            &self.number
        } else {
            &self.name
        }
    }
}

#[derive(Clone, Default)]
struct Folder {
    name: String,
    count: u32,
}

#[derive(Default, PartialEq)]
enum Screen {
    #[default]
    Loading,
    List,
    Folders,
    MoveTo,
    ConfirmDelete,
    Playing,
    Error,
}

/// What a request changes in the message selected or acted on.
enum Change {
    Remove,
    /// Marks it read (`true`) or unread.
    Mark(bool),
    /// Moves it to the folder.
    Move(String),
}

/// What a pending request is for.
#[derive(Clone, Copy, PartialEq)]
enum Want {
    List,
    Folders,
    Play,
    /// Delete, move, or mark: reload the list afterwards.
    Change,
}

struct Voicemail {
    folder: String,
    folders: Vec<Folder>,
    messages: Vec<Msg>,
    screen: Screen,
    list: Widget,
    pending: Vec<(u32, Want)>,
    /// Message being played or acted on.
    current: Option<Msg>,
    call: Option<u32>,
    shown: bool,
}

impl Default for Voicemail {
    fn default() -> Self {
        Voicemail {
            folder: "INBOX".into(),
            folders: Vec::new(),
            messages: Vec::new(),
            screen: Screen::Loading,
            list: 0,
            pending: Vec::new(),
            current: None,
            call: None,
            shown: false,
        }
    }
}

fn folder_title(name: &str) -> String {
    match name {
        "INBOX" => "Inbox".to_string(),
        other => other.to_string(),
    }
}

impl Voicemail {
    fn ask(&mut self, want: Want, id: u32) {
        self.pending.push((id, want));
    }

    fn loading(&mut self) {
        Message::show("Voicemail", "Loading...", &[Softkey::new(SK_EXIT, "Exit")]);
        self.screen = Screen::Loading;
    }

    fn load_list(&mut self) {
        let id = pbx::voicemail_messages(&self.folder, VoicemailSort::Date, true, PAGE, 1);
        self.ask(Want::List, id);
    }

    fn load_folders(&mut self) {
        self.ask(Want::Folders, pbx::voicemail_folders());
    }

    fn selected(&self) -> Option<Msg> {
        if self.screen != Screen::List || self.messages.is_empty() {
            return None;
        }
        self.messages.get(ui::selected(self.list)? as usize).cloned()
    }

    fn list_keys(&self) -> Vec<Softkey> {
        let mark = match self.selected() {
            Some(m) if m.read => "Unread",
            _ => "Read",
        };
        vec![
            Softkey::new(SK_PLAY, "Play"),
            Softkey::new(SK_DELETE, "Delete"),
            Softkey::new(SK_FOLDERS, "Folders"),
            Softkey::new(SK_MARK, mark),
            Softkey::new(SK_MOVE, "Move"),
            Softkey::new(SK_CALL, "Call Back"),
            Softkey::new(SK_EXIT, "Exit"),
        ]
    }

    fn draw_list(&mut self) {
        let r = screen(&folder_title(&self.folder));
        if self.messages.is_empty() {
            opendeskphone_app_sdk::nav::note(r, "No messages.");
            ui::set_softkeys(
                ROOT,
                &[
                    Softkey::new(SK_FOLDERS, "Folders"),
                    Softkey::new(SK_REFRESH, "Refresh"),
                    Softkey::new(SK_EXIT, "Exit"),
                ],
            );
            self.screen = Screen::List;
            self.list = 0;
            return;
        }
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = self
            .messages
            .iter()
            .map(|m| {
                let mark = if m.read { "" } else { "* " };
                ListItem::with_detail(
                    format!("{mark}{}", m.who()),
                    l10n::format_time(m.date, TimeStyle::DateAndTime),
                )
            })
            .collect();
        ui::set_items(list, &items);
        ui::set_focus(list);
        self.list = list;
        self.screen = Screen::List;
        ui::set_softkeys(ROOT, &self.list_keys());
    }

    fn draw_folders(&mut self, moving: bool) {
        let r = screen(if moving { "Move To" } else { "Folders" });
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = self
            .folders
            .iter()
            .map(|f| ListItem::with_detail(folder_title(&f.name), f.count.to_string()))
            .collect();
        ui::set_items(list, &items);
        ui::set_softkeys(
            ROOT,
            &[
                Softkey::new(SK_OPEN, if moving { "Move" } else { "Open" }),
                Softkey::new(SK_BACK, "Back"),
            ],
        );
        ui::set_focus(list);
        self.list = list;
        self.screen = if moving {
            Screen::MoveTo
        } else {
            Screen::Folders
        };
    }

    fn play(&mut self) {
        let Some(m) = self.selected() else { return };
        self.ask(Want::Play, pbx::voicemail_play(&self.folder, &m.id));
        let text = format!(
            "{}\n{}\n{}:{:02}\n\nConnecting...",
            m.who(),
            l10n::format_time(m.date, TimeStyle::DateAndTime),
            m.duration / 60,
            m.duration % 60
        );
        let mut keys = vec![Softkey::new(SK_STOP, "Stop")];
        if !has_keys() {
            // What `*`, `0`, and `#` do on a phone that has them.
            keys.extend([
                Softkey::new(SK_REWIND, "Rewind"),
                Softkey::new(SK_PAUSE, "Pause"),
                Softkey::new(SK_SKIP, "Skip"),
            ]);
        }
        Message::show("Playing", &text, &keys);
        ui::claim_keys(ROOT, &[Key::Cancel, Key::Star, Key::Pound, Key::Digit(0)]);
        self.current = Some(m);
        self.screen = Screen::Playing;
    }

    /// Sends the PBX's playback a digit: `*` rewinds, `#` skips, and `0`
    /// pauses.
    fn control(&mut self, digit: &str) {
        if self.screen == Screen::Playing
            && let Some(call) = self.call
        {
            let _ = calls::send_dtmf(call, digit);
        }
    }

    fn stop_playing(&mut self) {
        if let Some(call) = self.call.take() {
            let _ = calls::hangup(call);
        }
        self.after_playing();
    }

    fn after_playing(&mut self) {
        self.call = None;
        if self.screen != Screen::Playing {
            return;
        }
        // Listening marks the message read, as the stock app does.
        match self.current.take() {
            Some(m) if !m.read => {
                self.loading();
                let id = pbx::voicemail_mark_read(&self.folder, &[m.id]);
                self.ask(Want::Change, id);
            }
            _ => self.draw_list(),
        }
    }

    fn change(&mut self, change: Change) {
        let Some(m) = self.current.take().or_else(|| self.selected()) else {
            return;
        };
        let ids = [m.id];
        self.loading();
        let id = match change {
            Change::Remove => pbx::voicemail_remove(&self.folder, &ids),
            Change::Mark(true) => pbx::voicemail_mark_read(&self.folder, &ids),
            Change::Mark(false) => pbx::voicemail_mark_unread(&self.folder, &ids),
            Change::Move(to) => pbx::voicemail_move(&self.folder, &to, &ids),
        };
        self.ask(Want::Change, id);
    }

    fn error(&mut self, e: &PbxError) {
        Message::show(
            "Voicemail",
            &e.to_string(),
            &[
                Softkey::new(SK_REFRESH, "Retry"),
                Softkey::new(SK_EXIT, "Exit"),
            ],
        );
        self.screen = Screen::Error;
    }

    fn on_pbx(&mut self, id: u32, outcome: Result<PbxResult, PbxError>) {
        let Some(pos) = self.pending.iter().position(|(i, _)| *i == id) else {
            return;
        };
        let (_, want) = self.pending.remove(pos);
        let result = match outcome {
            Ok(result) => result,
            Err(e) => {
                if want == Want::Play {
                    self.current = None;
                }
                if self.shown {
                    self.error(&e);
                }
                return;
            }
        };
        match (want, result) {
            (Want::List, PbxResult::Voicemail(page)) => {
                self.messages = page.messages.into_iter().map(Msg::from).collect();
                if self.shown
                    && matches!(self.screen, Screen::Loading | Screen::List | Screen::Error)
                {
                    self.draw_list();
                }
            }
            (Want::Folders, PbxResult::VoicemailFolders(folders)) => {
                self.folders = folders
                    .into_iter()
                    .map(|f| Folder {
                        name: f.name,
                        count: f.count,
                    })
                    .collect();
            }
            // The call arrives as a push; nothing to do with the reply.
            (Want::Play, _) => {}
            (Want::Change, _) => {
                self.load_list();
                self.load_folders();
            }
            _ => {}
        }
    }
}

impl App for Voicemail {
    fn start(&mut self) {
        if let Err(e) = events::observe(PLAYBACK_EVENT) {
            opendeskphone_app_sdk::log_warn(&format!("cannot observe {PLAYBACK_EVENT}: {e:?}"));
        }
        calls::observe(true);
        // Stay loaded so a playback call that arrives late is still handled.
        runtime::set_exit_after_background(false);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                self.shown = true;
                self.folder = "INBOX".into();
                self.loading();
                self.load_list();
                self.load_folders();
            }
            Event::Background => {
                self.shown = false;
                if self.screen == Screen::Playing {
                    if let Some(call) = self.call.take() {
                        let _ = calls::hangup(call);
                    }
                    self.current = None;
                    self.screen = Screen::Loading;
                }
            }
            Event::Pbx((id, outcome)) => self.on_pbx(id, outcome),
            Event::Push(p) if p.name == PLAYBACK_EVENT => {
                let Some(call) = p.call else { return };
                if self.shown && self.screen == Screen::Playing {
                    if calls::answer(call).is_ok() {
                        self.call = Some(call);
                    }
                } else {
                    let _ = calls::reject(call);
                }
            }
            Event::Call(c) => {
                if self.call == Some(c.id) && c.state == opendeskphone_app_sdk::CallState::Ended {
                    self.after_playing();
                }
            }
            Event::Mwi(_) => {
                if self.shown && self.screen == Screen::List {
                    self.load_list();
                }
            }
            Event::Key(k) => match (k, &self.screen) {
                // Rewind, skip, and pause.
                (Key::Star, Screen::Playing) => self.control("*"),
                (Key::Pound, Screen::Playing) => self.control("#"),
                (Key::Digit(0), Screen::Playing) => self.control("0"),
                (Key::Cancel, Screen::Playing) => self.stop_playing(),
                (Key::Cancel, Screen::Folders | Screen::MoveTo | Screen::ConfirmDelete) => {
                    self.current = None;
                    self.draw_list()
                }
                (Key::Cancel, _) => runtime::background(),
                _ => {}
            },
            Event::Widget((_, WidgetEvent::Highlighted(_))) => {
                if self.screen == Screen::List {
                    ui::set_softkeys(ROOT, &self.list_keys());
                }
            }
            Event::Widget((_, WidgetEvent::Activated(i))) => match self.screen {
                // Touch: the touch that activates a message is the one
                // that highlights it, and the softkeys are for the
                // highlighted one.  "Play" plays it.
                Screen::List if !has_keys() => {}
                Screen::List => self.play(),
                Screen::Folders => {
                    if let Some(f) = self.folders.get(i as usize) {
                        self.folder = f.name.clone();
                        self.loading();
                        self.load_list();
                    }
                }
                Screen::MoveTo => {
                    if let Some(f) = self.folders.get(i as usize).cloned() {
                        self.change(Change::Move(f.name));
                    }
                }
                _ => {}
            },
            Event::Softkey(SK_PLAY) => self.play(),
            Event::Softkey(SK_STOP) => self.stop_playing(),
            Event::Softkey(SK_REWIND) => self.control("*"),
            Event::Softkey(SK_PAUSE) => self.control("0"),
            Event::Softkey(SK_SKIP) => self.control("#"),
            Event::Softkey(SK_DELETE) => {
                if let Some(m) = self.selected() {
                    Message::show(
                        "Delete",
                        &format!("Delete the message from {}?", m.who()),
                        &[Softkey::new(SK_YES, "Yes"), Softkey::new(SK_NO, "No")],
                    );
                    self.current = Some(m);
                    self.screen = Screen::ConfirmDelete;
                }
            }
            Event::Softkey(SK_YES) => self.change(Change::Remove),
            Event::Softkey(SK_NO | SK_BACK) => {
                self.current = None;
                self.draw_list();
            }
            Event::Softkey(SK_MARK) => {
                if let Some(m) = self.selected() {
                    self.change(Change::Mark(!m.read));
                }
            }
            Event::Softkey(SK_MOVE) => {
                if let Some(m) = self.selected() {
                    self.current = Some(m);
                    self.draw_folders(true);
                }
            }
            Event::Softkey(SK_FOLDERS) => self.draw_folders(false),
            Event::Softkey(SK_OPEN) => {
                if let Some(i) = ui::selected(self.list) {
                    self.handle(Event::Widget((self.list, WidgetEvent::Activated(i))));
                }
            }
            Event::Softkey(SK_CALL) => {
                if let Some(m) = self.selected()
                    && !m.number.is_empty()
                    && calls::dial(&m.number, None, &[]).is_ok()
                {
                    runtime::background();
                }
            }
            Event::Softkey(SK_REFRESH) => {
                self.loading();
                self.load_list();
                self.load_folders();
            }
            Event::Softkey(SK_EXIT) => runtime::background(),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Voicemail,
    name: "voicemail",
    id: "org.opendeskphone.voicemail",
    display_name: "Voicemail",
    description: "Listen to and manage voicemail messages",
    phone_icon: Some(Icon::AppVoicemail),
}
