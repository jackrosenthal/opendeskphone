//! The SDK for Open Desk Phone apps (the README tells how to build, package,
//! and install one).
//!
//! An app is a `cdylib` built for `wasm32-unknown-unknown` that implements
//! [`App`] and registers it with [`export_app!`]:
//!
//! ```ignore
//! use opendeskphone_app_sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct Hello;
//!
//! impl App for Hello {
//!     fn start(&mut self) {
//!         let t = ui::create(WidgetKind::Text, ROOT, Rect::new(0, 0, 200, 20));
//!         ui::set_text(t, "Hello");
//!     }
//!     fn handle(&mut self, _e: Event) {}
//! }
//!
//! opendeskphone_app_sdk::export_app!(Hello);
//! ```
//!
//! The interface is defined once, in `wit/app.wit` in this crate; the
//! modules re-exported here ([`ui`], [`pbx`], [`calls`], ...) are generated
//! from it.  An app is given the interfaces its component imports, and no
//! others.

#![allow(clippy::too_many_arguments)]

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "app",
        pub_export_macro: true,
        default_bindings_module: "opendeskphone_app_sdk::bindings",
        additional_derives: [PartialEq],
        generate_all,
    });
}

pub mod controls;
pub mod manifest;
pub mod nav;
pub mod xml;

pub use bindings::opendeskphone::app::types::{
    AccountInfo, Align, CallInfo, CallQueue, CallState, Error, Event, ExpansionKey, Font, Header,
    HttpResponse, Icon, Image, InputMode, Key, LineState, ListItem, Metrics, Mwi,
    ParkedCall, ParkingLot, PbxError, PbxPresence, PbxPresenceOption, PbxPresenceState, PbxResult,
    PbxServerError, PbxUser, PhoneModel, Point, Presence, PresenceStatus, Push, PushMethod, QueueCaller,
    QueueMember, QueueMembership, QueueStatus, Rect, Size, Skin, Softkey, StorageChange, StorageEntry,
    StorageLocation, StorageOutcome, StorageReply, Touch, VoicemailFolder, VoicemailMessage,
    VoicemailPage, VoicemailSort, WidgetEvent, WidgetKind,
};
pub use bindings::opendeskphone::app::l10n::TimeStyle;
pub use bindings::opendeskphone::app::{
    alarms, audio, calls, canvas, config, diagnostics, events, hotdesk, http, input, l10n, lamps, pbx,
    presence, runtime, storage, timers, ui,
};

/// A widget handle.
pub type Widget = u32;
/// The app window's root widget.
pub const ROOT: Widget = 0;

/// What an app implements.
pub trait App: Default + 'static {
    /// Called once after the app is loaded.
    fn start(&mut self);
    /// Called for every event.
    fn handle(&mut self, event: Event);
}

impl Rect {
    pub const fn new(x: i16, y: i16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl ListItem {
    pub fn new(text: impl Into<String>) -> ListItem {
        ListItem {
            text: text.into(),
            detail: String::new(),
            icon: None,
        }
    }
    pub fn with_detail(text: impl Into<String>, detail: impl Into<String>) -> ListItem {
        ListItem {
            text: text.into(),
            detail: detail.into(),
            icon: None,
        }
    }
}

impl Softkey {
    pub fn new(id: u32, label: impl Into<String>) -> Softkey {
        Softkey {
            id,
            label: label.into(),
        }
    }
}

/// Log helpers.
pub fn log_error(msg: &str) {
    runtime::log(0, msg);
}
pub fn log_warn(msg: &str) {
    runtime::log(1, msg);
}
pub fn log_info(msg: &str) {
    runtime::log(2, msg);
}
pub fn log_debug(msg: &str) {
    runtime::log(3, msg);
}

/// Has a panic written to the phone's log at error level, with the file
/// and line, before the panic stops the app.  [`export_app!`] calls this
/// when the app starts.  The phone keeps the first 512 bytes of a log
/// message.
#[doc(hidden)]
pub fn log_panics() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.payload_as_str().unwrap_or("(no message)");
        let text = match info.location() {
            Some(at) => format!("panicked at {}:{}: {message}", at.file(), at.line()),
            None => format!("panicked: {message}"),
        };
        runtime::log(0, &text);
    }));
}

/// The full content area as a rectangle at the origin.
pub fn content_rect() -> Rect {
    let m = ui::get_metrics();
    Rect::new(0, 0, m.width, m.height)
}

/// Row height of the skin.
pub fn row_height() -> u16 {
    ui::get_metrics().row_height
}

/// Whether the phone has keys.  One without (the touchscreen, the D80) is
/// used by touch alone: what an app does with a key there, it offers on
/// the screen instead.
pub fn has_keys() -> bool {
    ui::get_metrics().keys
}

/// Clears the window, draws a title line, and returns the rectangle left
/// below it.  The window claims `cancel`, so the app sees it as
/// [`Event::Key`] and decides between "back" and leaving.
///
/// On the touchscreen the title line is the title bar every screen of the
/// phone has.  The phone paints the bar across the top `title_height`
/// pixels of the window, under the app's widgets, and draws its two
/// buttons over the ends, each `back_width` wide: back at the left, which
/// is `cancel`, and home at the right, which leaves the app.  The title
/// is between them, and has the bar's text color unless it is given one.
/// An app that lays out its own title line keeps to the same: nothing of
/// its own under the two buttons, and no background on what it puts in
/// the bar.  The title has the focus there, so that a list made after it
/// shows no highlighted row until a row is touched: see
/// [`nav::focus_list`].
pub fn screen(title: &str) -> Rect {
    ui::clear(ROOT);
    ui::claim_keys(ROOT, &[Key::Cancel]);
    let m = ui::get_metrics();
    let t = ui::create(
        WidgetKind::Text,
        ROOT,
        Rect::new(
            m.back_width as i16,
            0,
            m.width.saturating_sub(2 * m.back_width),
            m.title_height,
        ),
    );
    ui::set_font(t, Font::Title);
    ui::set_text(t, title);
    if !m.keys {
        ui::set_focus(t);
    }
    Rect::new(
        0,
        m.title_height as i16,
        m.width,
        m.height.saturating_sub(m.title_height),
    )
}

/// The value of a `local` storage key as UTF-8 text.
pub fn read_string(key: &str) -> Option<String> {
    match storage::get(StorageLocation::Local, key) {
        StorageReply::Done(StorageOutcome::Entry(Some(e))) => String::from_utf8(e.value).ok(),
        _ => None,
    }
}

pub mod prelude {
    pub use crate::manifest::AppKind;
    pub use crate::nav::{Form, Message};
    pub use crate::{
        App, Event, Icon, Image, Key, ListItem, PbxError, PbxResult, Point, ROOT, Rect, Skin,
        Softkey, StorageEntry, StorageLocation, StorageOutcome, StorageReply, TimeStyle, Touch,
        Widget, WidgetEvent, WidgetKind, audio, calls, config, content_rect, events, has_keys,
        http, l10n, pbx, presence, row_height, runtime, screen, storage, timers, ui,
    };
}

/// Registers the app with the phone: the type that implements [`App`], and
/// the app's manifest (the fields of [`manifest::Manifest`]), which goes
/// into the app's WebAssembly file.
///
/// ```ignore
/// opendeskphone_app_sdk::export_app! {
///     app: Hello,
///     name: "hello",
///     display_name: "Hello",
///     description: "Says hello",
/// }
/// ```
///
/// `app` comes first; `name` and `display_name` must be given; the other
/// fields may be: `id`, `description`, `kind`, `has_idle`, `hidden`, `icon`,
/// `icon_for`, `phone_icon`, and `memory_kib`.  A field the phone would
/// refuse does not compile.
///
/// When the app starts, the macro has a panic of the app written to the
/// phone's log, with the place in the code, before the phone stops the
/// app.
#[macro_export]
macro_rules! export_app {
    (app: $ty:ty, $($field:ident : $value:expr),+ $(,)?) => {
        const _: () = {
            const MANIFEST: $crate::manifest::Manifest = $crate::manifest::Manifest {
                $($field: $value,)+
                ..$crate::manifest::Manifest::EMPTY
            }
            .checked();
            const LEN: usize = MANIFEST.encoded_len();
            // A custom section of the WebAssembly module.
            #[cfg_attr(target_family = "wasm", unsafe(link_section = "opendeskphone-app"))]
            #[used]
            static SECTION: [u8; LEN] = MANIFEST.encode::<LEN>();
        };

        #[doc(hidden)]
        mod __opendeskphone_app_export {
            use super::*;
            use core::cell::RefCell;
            struct Cell(RefCell<Option<$ty>>);
            // SAFETY: a WebAssembly app instance is single-threaded.
            unsafe impl Sync for Cell {}
            static APP: Cell = Cell(RefCell::new(None));
            pub struct Shim;
            impl $crate::bindings::exports::opendeskphone::app::guest::Guest for Shim {
                fn start() {
                    $crate::log_panics();
                    let mut slot = APP.0.borrow_mut();
                    let app = slot.insert(<$ty as Default>::default());
                    $crate::App::start(app);
                }
                fn handle(e: $crate::Event) {
                    if let Some(app) = APP.0.borrow_mut().as_mut() {
                        $crate::App::handle(app, e);
                    }
                }
            }
            $crate::bindings::export!(Shim with_types_in $crate::bindings);
        }
    };
    ($ty:ty) => {
        compile_error!(
            "export_app! takes the app's manifest too: export_app! { app: Type, name: \"name\", display_name: \"Name\" }"
        );
    };
}

/// Text for a failed PBX request, for an error screen: the server's
/// message, or that the server could not be reached and why.
impl core::fmt::Display for PbxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PbxError::Server(e) => f.write_str(&e.message),
            PbxError::Transport(why) => write!(f, "Could not reach the server.\n{why}"),
        }
    }
}
