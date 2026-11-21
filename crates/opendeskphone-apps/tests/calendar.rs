//! The Calendar app on the touchscreen (D80), which has no keys: the
//! month is turned by a swipe or by the buttons beside its name, and the
//! back button leaves.  The calendar on the phones with physical buttons is tested in
//! `apps.rs`.

mod common;

use common::{Rig, staged};
use opendeskphone_apps::DrawOp;
use opendeskphone_apps::testutil::FakeConfig;
use opendeskphone_state::{Key, Model};

const APP: &str = "calendar";

/// 2026-11-14 01:00:00 UTC, which is the evening of November 13 seven
/// hours west.
const NOV_14_0100_UTC: u64 = 1_794_618_000;

fn rig(model: Model) -> Rig {
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(NOV_14_0100_UTC));
    let config = FakeConfig { utc_offset: -7 * 3600, wall_time: Some(clock), ..Default::default() };
    let mut r = Rig::with_root(staged(), config, model);
    r.launch(APP);
    r
}

/// The month's name: under the screen's title on the touchscreen.
fn month(r: &Rig) -> String {
    let texts = r.ui.texts(APP);
    assert_eq!(texts[0], "Calendar");
    texts[1].clone()
}

/// The day on today's disc, if one is shown.
fn today(r: &Rig) -> Option<String> {
    r.ui.drawings(APP).into_iter().find_map(|(_, ops)| {
        ops.iter().find_map(|op| match op {
            DrawOp::Text(t) => Some(t.text.clone()),
            _ => None,
        })
    })
}

fn swipe(r: &mut Rig, from: (i32, i32), to: (i32, i32)) {
    r.ui.drag(APP, from, to);
    r.settle();
}

/// The buttons beside the month's name: the month before, and the one
/// after (96 pixels square, under the title, 16 in from the sides).
fn previous(r: &mut Rig) {
    r.ui.tap(APP, 16 + 48, 96 + 8 + 48);
    r.settle();
}

fn next(r: &mut Rig) {
    r.ui.tap(APP, 800 - 16 - 48, 96 + 8 + 48);
    r.settle();
}

#[test]
fn on_the_touchscreen_a_swipe_or_a_button_turns_the_month() {
    let mut r = rig(Model::D80);
    assert_eq!(month(&r), "November 2026");
    assert_eq!(today(&r).as_deref(), Some("13"));
    assert!(r.ui.highlighted(APP).is_empty(), "today is a disc, not a highlighted cell");
    // The back button leaves, and the buttons turn the month: no
    // softkeys in the current month.
    assert_eq!(r.ui.softkeys(APP), [] as [&str; 0]);
    // The weekdays have room for their names.
    assert_eq!(r.ui.texts(APP)[2..9], ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);

    // To the left: the next month comes in; to the right, the one before.
    swipe(&mut r, (600, 500), (200, 520));
    assert_eq!(month(&r), "December 2026");
    assert_eq!(today(&r), None);
    // Away from the current month, "Today" goes back to it.
    assert_eq!(r.ui.softkeys(APP), ["Today"]);
    swipe(&mut r, (600, 700), (300, 650));
    assert_eq!(month(&r), "January 2027");
    swipe(&mut r, (100, 300), (500, 300));
    swipe(&mut r, (100, 300), (500, 300));
    swipe(&mut r, (100, 300), (500, 300));
    assert_eq!(month(&r), "October 2026");
    // Up and down, a short way, and a tap on a day turn nothing.
    swipe(&mut r, (400, 300), (450, 900));
    swipe(&mut r, (400, 900), (350, 300));
    swipe(&mut r, (400, 300), (440, 300));
    r.ui.tap(APP, 400, 600);
    r.settle();
    assert_eq!(month(&r), "October 2026");
    // The buttons beside the month's name.
    next(&mut r);
    assert_eq!(month(&r), "November 2026");
    assert_eq!(r.ui.softkeys(APP), [] as [&str; 0]);
    previous(&mut r);
    previous(&mut r);
    assert_eq!(month(&r), "September 2026");
    r.softkey(APP, "Today");
    assert_eq!(month(&r), "November 2026");
    assert_eq!(today(&r).as_deref(), Some("13"));
    // The back button is the key `cancel`.
    r.key(APP, Key::Cancel);
    assert_ne!(r.ui.top().as_deref(), Some(APP));
}

#[test]
fn a_phone_with_keys_takes_no_touches() {
    let mut r = rig(Model::D60);
    assert_eq!(r.ui.softkeys(APP), ["Prev", "Today", "Next", "Exit"]);
    assert!(!r.ui.takes_touches(APP, 0));
    swipe(&mut r, (400, 100), (100, 100));
    assert_eq!(r.ui.texts(APP)[0], "November 2026");
    assert!(r.ui.drawings(APP).is_empty(), "no buttons, and no disc");
}
