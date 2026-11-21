//! Hot desking: log a user in to this phone with an extension and PIN, or
//! log the current user out.  The phone reprovisions for the new user, and
//! the app stays on screen until the phone says how that ended.
//!
//! On the touchscreen the form opens with the extension's input focused
//! and the on-screen keyboard up.  The keyboard's key that ends an input
//! goes on to the PIN, and from the PIN logs in: the extension, that key,
//! the PIN, and that key again are all it takes.  (The app moves on and
//! logs in on `WidgetEvent::Submitted`, whether the keyboard has moved the
//! focus on itself or not.)  With the keyboard put away, "Log in" is a
//! button across the bottom; "Log out" and "Try again" are buttons too,
//! and logging out asks on a screen with "Cancel" and "Log out".

use opendeskphone_app_sdk::controls::{self, Button, Look};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{InputMode, hotdesk, nav};

const SK_LOGIN: u32 = 1;
const SK_LOGOUT: u32 = 2;
const SK_YES: u32 = 3;
const SK_NO: u32 = 4;
const SK_EXIT: u32 = 5;
const SK_BACK: u32 = 6;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    nav::softkeys(keys, &[SK_EXIT, SK_BACK])
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
    /// Touch: the screen's button, with the softkey whose work it does,
    /// and the question before logging out.
    button: Option<(Button, u32)>,
    confirm: Option<nav::Confirm>,
}

impl HotDesk {
    /// Touch: a button across the bottom of the window that does what the
    /// softkey `id` does.
    fn button(&mut self, label: &str, look: Look, id: u32) {
        let (_, bottom) = controls::split_bottom(content_rect());
        self.button = Some((Button::new(ROOT, bottom, label, look), id));
    }

    fn show(&mut self) {
        self.button = None;
        self.confirm = None;
        let user = hotdesk::current_user();
        if user.is_empty() {
            // (Touch: "Log in" is a button.)
            let softkeys = if has_keys() {
                vec![Softkey::new(SK_LOGIN, "Log In"), Softkey::new(SK_EXIT, "Cancel")]
            } else {
                Vec::new()
            };
            let form = Form::show(
                "Log In",
                &[
                    ("Extension", InputMode::Numeric, ""),
                    ("PIN", InputMode::Pin, ""),
                ],
                &softkeys,
            );
            if !has_keys() {
                self.button("Log in", Look::Primary, SK_LOGIN);
            }
            self.screen = Screen::Login(form);
        } else {
            Message::show(
                "Hot Desking",
                &format!("Logged in as {user}."),
                &if has_keys() {
                    vec![Softkey::new(SK_LOGOUT, "Log Out"), Softkey::new(SK_EXIT, "Exit")]
                } else {
                    Vec::new()
                },
            );
            if !has_keys() {
                self.button("Log out", Look::Primary, SK_LOGOUT);
            }
            self.screen = Screen::LoggedIn;
        }
    }

    fn fail(&mut self, text: &str) {
        self.button = None;
        Message::show("Hot Desking", text, &keys(&[Softkey::new(SK_BACK, "Back")]));
        if !has_keys() {
            self.button("Try again", Look::Primary, SK_BACK);
        }
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
        self.button = None;
        self.confirm = None;
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
            Err(why) if has_keys() => self.fail(&format!("The PBX did not allow that:\n{why}")),
            Err(why) => self.fail(&format!(
                "The PBX did not allow that: {why}\nCheck the extension and the PIN, and try again."
            )),
        }
    }

    fn ask_logout(&mut self) {
        let question = format!("Log {} out of this phone?", hotdesk::current_user());
        if has_keys() {
            Message::show("Log Out", &question, &keys(&[Softkey::new(SK_YES, "Yes"), Softkey::new(SK_NO, "No")]));
        } else {
            self.button = None;
            self.confirm = Some(nav::Confirm::show("Log Out", &question, "Log out", Look::Primary));
        }
        self.screen = Screen::Confirm;
    }

    /// Touch: what a finger did on a widget that takes touches.
    fn touched(&mut self, w: Widget, t: &Touch) {
        if let Some(yes) = self.confirm.as_mut().and_then(|c| c.touched(w, t)) {
            return self.handle(Event::Softkey(if yes { SK_YES } else { SK_NO }));
        }
        let tapped = self.button.as_mut().and_then(|(b, id)| b.touched(w, t).then_some(*id));
        if let Some(id) = tapped {
            self.handle(Event::Softkey(id));
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
            Event::Touch((w, t)) => self.touched(w, &t),
            Event::Softkey(SK_LOGOUT) => self.ask_logout(),
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
