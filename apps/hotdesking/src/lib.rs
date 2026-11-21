//! Hot desking: log a user in to this phone with an extension and PIN, or
//! log the current user out.  The phone reprovisions for the new user, and
//! the app stays on screen until the phone says how that ended.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{InputMode, hotdesk};

const SK_LOGIN: u32 = 1;
const SK_LOGOUT: u32 = 2;
const SK_YES: u32 = 3;
const SK_NO: u32 = 4;
const SK_EXIT: u32 = 5;
const SK_BACK: u32 = 6;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_BACK])
}

#[derive(Default)]
enum Screen {
    #[default]
    None,
    Login(Form),
    LoggedIn,
    Confirm,
    /// Waiting for the phone to say how a login or logout ended.
    Working,
    Failed,
}

#[derive(Default)]
struct HotDesk {
    screen: Screen,
}

impl HotDesk {
    fn show(&mut self) {
        let user = hotdesk::current_user();
        if user.is_empty() {
            let form = Form::show(
                "Log In",
                &[
                    ("Extension", InputMode::Numeric, ""),
                    ("PIN", InputMode::Pin, ""),
                ],
                &keys(&[
                    Softkey::new(SK_LOGIN, "Log In"),
                    Softkey::new(SK_EXIT, "Cancel"),
                ]),
            );
            self.screen = Screen::Login(form);
        } else {
            Message::show(
                "Hot Desking",
                &format!("Logged in as {user}."),
                &keys(&[
                    Softkey::new(SK_LOGOUT, "Log Out"),
                    Softkey::new(SK_EXIT, "Exit"),
                ]),
            );
            self.screen = Screen::LoggedIn;
        }
    }

    fn fail(&mut self, text: &str) {
        Message::show("Hot Desking", text, &keys(&[Softkey::new(SK_BACK, "Back")]));
        self.screen = Screen::Failed;
    }

    fn login(&mut self) {
        let Screen::Login(form) = &self.screen else {
            return;
        };
        let (ext, pin) = (form.value(0), form.value(1));
        if ext.is_empty() || pin.is_empty() {
            return;
        }
        match hotdesk::login(&ext, &pin) {
            Ok(()) => self.wait(&format!("Logging in {ext}...")),
            Err(_) => self.fail("The phone could not start the login."),
        }
    }

    /// Shows what the phone is doing until `Event::HotdeskDone` comes.
    fn wait(&mut self, text: &str) {
        Message::show("Hot Desking", text, &keys(&[Softkey::new(SK_EXIT, "Exit")]));
        self.screen = Screen::Working;
    }

    /// The phone said how the login or logout ended.
    fn done(&mut self, result: Result<(), String>) {
        if !matches!(self.screen, Screen::Working) {
            return;
        }
        match result {
            // Logged in: who the phone is now for.  Logged out: the form.
            Ok(()) => self.show(),
            Err(why) => self.fail(&format!("The PBX did not allow that:\n{why}")),
        }
    }
}

impl App for HotDesk {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => self.show(),
            Event::Softkey(SK_LOGIN) => self.login(),
            Event::Widget((w, WidgetEvent::Submitted(_))) => {
                let advanced = match &self.screen {
                    Screen::Login(form) => form.focus_next(w),
                    _ => true,
                };
                if !advanced {
                    self.login();
                }
            }
            Event::Softkey(SK_LOGOUT) => {
                Message::show(
                    "Log Out",
                    &format!("Log {} out of this phone?", hotdesk::current_user()),
                    &keys(&[Softkey::new(SK_YES, "Yes"), Softkey::new(SK_NO, "No")]),
                );
                self.screen = Screen::Confirm;
            }
            Event::Softkey(SK_YES) => match hotdesk::logout() {
                Ok(()) => self.wait("Logging out..."),
                Err(_) => self.fail("The phone could not log out."),
            },
            Event::HotdeskDone(result) => self.done(result),
            Event::Softkey(SK_NO | SK_BACK) => self.show(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => match self.screen {
                Screen::Confirm | Screen::Failed => self.show(),
                _ => runtime::background(),
            },
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: HotDesk,
    name: "hotdesking",
    id: "org.opendeskphone.hotdesking",
    display_name: "Hot Desking",
    description: "Log in to or out of this phone",
    phone_icon: Some(Icon::AppHotdesking),
}
