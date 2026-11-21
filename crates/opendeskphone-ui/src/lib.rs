//! Open Desk Phone's phone UI: a small retained-mode toolkit with integer-only
//! rendering, three skins, the native screens, and the view API that WASM
//! apps draw through (plan section 10).
//!
//! - [`Ui`] owns the display, backlight, keypad, and lamps.  The
//!   integrator builds one with [`Ui::from_hal`] and calls [`Ui::run`] on
//!   a dedicated thread.  It watches the [`opendeskphone_state::Store`], draws the
//!   screen that the state calls for, turns keys into
//!   [`opendeskphone_state::Command`]s, re-emits input as
//!   [`opendeskphone_state::Event`]s, handles the `Domain::Ui` commands, and
//!   drives lamps from `Keys` state.
//! - [`UiHandle`] is the thread-safe way in: open and close app views,
//!   change [`UiConfig`], stop the loop.
//! - [`appview`] is the widget-tree API for apps; see its module docs.
//! - [`gfx`], [`font`], [`skin`], and [`widgets`] are the toolkit, public
//!   for tools and tests.
//! - [`wallpaper`] has the color skin's built-in wallpapers.
//! - [`touch`] is the touch skin's input (D80): the regions of the screen
//!   that can be touched, and the gestures of a finger.
//! - [`osk`] is the touch skin's on-screen keyboard, for any screen that
//!   takes text.
//!
//! The touch skin is the feature `touch`, which is on by default.  Without
//! it (the firmware of the phones with keys, which have little flash) the
//! build has none of that skin's fonts, icons, and artwork, and
//! [`skin::Theme::touch`] is false at compile time, so the compiler leaves
//! out what the screens do only by touch, [`osk`] included.  The API is
//! the same either way.
//!
//! Fonts and artwork are rasterized at build time (`build.rs`) from
//! `assets/`; drawing at run time uses no floating point.  The one file
//! format read at run time is PNG ([`image`]), for the icons and pictures
//! of app packages.

#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod appview;
mod appview_impl;
pub mod dialplan;
pub mod font;
pub mod gfx;
pub mod icons;
pub mod image;
pub mod multitap;
pub mod osk;
mod screens;
pub mod skin;
pub mod strings;
pub mod time;
pub mod touch;
mod ui;
pub mod wallpaper;
pub mod widgets;

#[allow(clippy::all)]
mod atlas {
    include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));
}

pub use appview::{AppEvent, AppEvents, AppView};
pub use touch::{Gesture, Hit};
pub use ui::{IdleSoftkey, Ui, UiConfig, UiHandle, TRIAL_COVER_MAX_SECS};

/// Size in pixels of the area on the idle screen offered to an app's idle
/// view ([`UiHandle::add_idle_view`]).  Widget rectangles in that view are
/// relative to this area, not to the app content area.
pub fn idle_app_area(model: opendeskphone_state::Model) -> opendeskphone_state::Size {
    let th = skin::Theme::for_model(model);
    opendeskphone_state::Size::new(screens::idle::idle_app_w(th) as u16, screens::idle::idle_app_h(th) as u16)
}
