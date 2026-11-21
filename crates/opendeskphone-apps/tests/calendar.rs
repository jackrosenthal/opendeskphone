//! The Calendar app on the touchscreen (D80), which has no keys: the
//! month is turned by a swipe or by the buttons, and the back button
//! leaves.  The calendar on the phones with keys is tested in `apps.rs`.

mod common;

use common::{Rig, staged};
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

fn title(r: &Rig) -> String {
    r.ui.texts(APP)[0].clone()
}

fn swipe(r: &mut Rig, from: (i32, i32), to: (i32, i32)) {
    r.ui.drag(APP, from, to);
    r.settle();
}

#[test]
fn on_the_touchscreen_a_swipe_turns_the_month() {
    let mut r = rig(Model::D80);
    assert_eq!(title(&r), "November 2026");
    assert_eq!(r.ui.highlighted(APP), ["13"]);
    // The back button leaves: no "Exit".
    assert_eq!(r.ui.softkeys(APP), ["Prev", "Today", "Next"]);
    // The weekdays have room for their names.
    assert_eq!(r.ui.texts(APP)[1..8], ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);

    // To the left: the next month comes in; to the right, the one before.
    swipe(&mut r, (600, 500), (200, 520));
    assert_eq!(title(&r), "December 2026");
    assert!(r.ui.highlighted(APP).is_empty());
    swipe(&mut r, (600, 700), (300, 650));
    assert_eq!(title(&r), "January 2027");
    swipe(&mut r, (100, 300), (500, 300));
    swipe(&mut r, (100, 300), (500, 300));
    swipe(&mut r, (100, 300), (500, 300));
    assert_eq!(title(&r), "October 2026");
    // Up and down, a short way, and a tap turn nothing.
    swipe(&mut r, (400, 300), (450, 900));
    swipe(&mut r, (400, 900), (350, 300));
    swipe(&mut r, (400, 300), (440, 300));
    r.ui.tap(APP, 400, 600);
    r.settle();
    assert_eq!(title(&r), "October 2026");
    // The buttons do as on every phone.
    r.softkey(APP, "Next");
    assert_eq!(title(&r), "November 2026");
    r.softkey(APP, "Prev");
    r.softkey(APP, "Prev");
    assert_eq!(title(&r), "September 2026");
    r.softkey(APP, "Today");
    assert_eq!(title(&r), "November 2026");
    assert_eq!(r.ui.highlighted(APP), ["13"]);
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
    assert_eq!(title(&r), "November 2026");
}
