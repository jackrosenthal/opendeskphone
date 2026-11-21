//! Parking: lists the calls parked in the user's parking lots and
//! retrieves one by dialing its space.

use opendeskphone_app_sdk::prelude::*;

const SK_RETRIEVE: u32 = 1;
const SK_REFRESH: u32 = 2;
const SK_EXIT: u32 = 3;

/// The softkeys the touchscreen does without: the back button is the way
/// out, and a touch retrieves a call.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_RETRIEVE])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}
const REFRESH_MS: u32 = 10_000;
/// Push the PBX sends to the parker's phone when a call is parked.
const PARKED_EVENT: &str = "digium.parkingLot.parked";

struct Parked {
    space: String,
    name: String,
    number: String,
    seconds: u32,
}

#[derive(Default)]
struct Parking {
    calls: Vec<Parked>,
    list: Option<Widget>,
    request: Option<u32>,
    timer: Option<u32>,
    shown: bool,
}

fn duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl Parking {
    fn refresh(&mut self) {
        self.request = Some(pbx::parking_lots());
    }

    fn draw(&mut self) {
        let r = screen("Parked Calls");
        if self.calls.is_empty() {
            // (Touch: with what to do, in the upper part of the screen.)
            let text = if has_keys() {
                "No calls are parked."
            } else {
                "No calls are parked.  A call parked in your lots shows here."
            };
            opendeskphone_app_sdk::nav::note(r, text);
            set_keys(&[
                Softkey::new(SK_REFRESH, "Refresh"),
                Softkey::new(SK_EXIT, "Exit"),
            ]);
            self.list = None;
            return;
        }
        let keep = self.list.and_then(ui::selected).unwrap_or(0);
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = self
            .calls
            .iter()
            .map(|c| {
                let who = if c.name.is_empty() {
                    c.number.clone()
                } else {
                    c.name.clone()
                };
                ListItem::with_detail(format!("{}  {who}", c.space), duration(c.seconds))
            })
            .collect();
        ui::set_items(list, &items);
        ui::set_selected(list, keep.min(items.len() as u32 - 1));
        set_keys(&[
            Softkey::new(SK_RETRIEVE, "Retrieve"),
            Softkey::new(SK_REFRESH, "Refresh"),
            Softkey::new(SK_EXIT, "Exit"),
        ]);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.list = Some(list);
    }

    fn retrieve(&mut self, index: usize) {
        if let Some(c) = self.calls.get(index)
            && calls::dial(&c.space, None, &[]).is_ok()
        {
            runtime::background();
        }
    }

    fn on_reply(&mut self, outcome: Result<PbxResult, PbxError>) {
        match outcome {
            Ok(result) => {
                self.calls.clear();
                let PbxResult::ParkingLots(lots) = result else {
                    return self.draw();
                };
                for c in lots.into_iter().flat_map(|lot| lot.calls) {
                    self.calls.push(Parked {
                        space: c.space,
                        name: c.caller_name,
                        number: c.caller_number,
                        seconds: c.duration,
                    });
                }
                self.calls.sort_by(|a, b| a.space.cmp(&b.space));
                self.draw();
            }
            Err(e) => {
                self.list = None;
                Message::show(
                    "Parked Calls",
                    &e.to_string(),
                    &keys(&[
                        Softkey::new(SK_REFRESH, "Retry"),
                        Softkey::new(SK_EXIT, "Exit"),
                    ]),
                );
            }
        }
    }
}

impl App for Parking {
    fn start(&mut self) {
        if let Err(e) = events::observe(PARKED_EVENT) {
            opendeskphone_app_sdk::log_warn(&format!("cannot observe {PARKED_EVENT}: {e:?}"));
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                self.shown = true;
                self.list = None;
                Message::show(
                    "Parked Calls",
                    "Loading...",
                    &keys(&[Softkey::new(SK_EXIT, "Exit")]),
                );
                self.refresh();
                self.timer = timers::set(REFRESH_MS, true).ok();
            }
            Event::Background | Event::Exit => {
                self.shown = false;
                if let Some(t) = self.timer.take() {
                    timers::cancel(t);
                }
            }
            Event::Timer(id) if self.timer == Some(id) => self.refresh(),
            Event::Push(p) if p.name == PARKED_EVENT && self.shown => self.refresh(),
            Event::Pbx((id, outcome)) if self.request == Some(id) => {
                self.request = None;
                if self.shown {
                    self.on_reply(outcome);
                }
            }
            Event::Widget((_, WidgetEvent::Activated(i))) => self.retrieve(i as usize),
            Event::Softkey(SK_RETRIEVE) => {
                if let Some(row) = self.list.and_then(ui::selected) {
                    self.retrieve(row as usize);
                }
            }
            Event::Softkey(SK_REFRESH) => self.refresh(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => runtime::background(),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Parking,
    name: "parking",
    id: "org.opendeskphone.parking",
    display_name: "Parking",
    description: "See and retrieve parked calls",
    phone_icon: Some(Icon::AppParking),
}
