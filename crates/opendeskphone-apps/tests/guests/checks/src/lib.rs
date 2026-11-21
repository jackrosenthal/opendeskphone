//! A test guest that does what a test asks of it and shows what the phone
//! answered.  A test asks with a message (`Event::Message`): the name is
//! what to do, the payload its argument, and the answer is the text of
//! the app's first widget.  The calls and pushes the app is told of are
//! the text of its second widget, in the order they came.

use opendeskphone_app_sdk::canvas::{Color, Op};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::lamps::Lamp;
use opendeskphone_app_sdk::{CallState, InputMode, PresenceStatus, PushMethod, alarms, canvas, hotdesk, lamps};

/// The app's data is in its code.
const TONE: &[u8] = include_bytes!("../../probe/data/tone.wav");
const PIC: &[u8] = include_bytes!("../../probe/data/pic.png");

#[derive(Default)]
struct Checks {
    out: Widget,
    seen: Widget,
    log: Vec<String>,
}

fn state(s: CallState) -> &'static str {
    match s {
        CallState::Dialing => "dialing",
        CallState::Proceeding => "proceeding",
        CallState::Ringback => "ringback",
        CallState::Ringing => "ringing",
        CallState::Active => "active",
        CallState::Held => "held",
        CallState::Transferring => "transferring",
        CallState::Ended => "ended",
    }
}

/// What a `local` storage request ended in, without the parts that
/// change from run to run (the version and the time).
fn stored(reply: StorageReply) -> String {
    match reply {
        StorageReply::Done(StorageOutcome::Stored(e)) => format!("stored {}", e.key),
        StorageReply::Done(StorageOutcome::Entry(e)) => {
            format!("entry {:?}", e.map(|e| String::from_utf8_lossy(&e.value).into_owned()))
        }
        StorageReply::Done(StorageOutcome::Refused(why)) => format!("refused: {why}"),
        StorageReply::Done(StorageOutcome::Unreachable(why)) => format!("unreachable: {why}"),
        other => format!("{other:?}"),
    }
}

/// How many times `f` succeeds before it fails, and the failure.
fn until_refused<E: core::fmt::Debug>(most: u32, mut f: impl FnMut(u32) -> Result<(), E>) -> String {
    for n in 0..most {
        if let Err(e) = f(n) {
            return format!("{n} then {e:?}");
        }
    }
    format!("{most} and no refusal")
}

fn fill() -> Vec<Op> {
    vec![Op::Fill((Rect::new(0, 0, 4, 4), Color { r: 255, g: 0, b: 0 }))]
}

/// The functions that wait for the phone's answer.
const WAITS: [&str; 13] = [
    "dial",
    "answer",
    "reject",
    "hangup",
    "send-dtmf",
    "dial-voicemail",
    "set-status",
    "login",
    "logout",
    "set-alarm",
    "remove-alarm",
    "dismiss",
    "play",
];

/// Calls the function of `WAITS` that `which` names, and shows the
/// result.
fn wait(which: &str) -> String {
    let alarm = alarms::Alarm {
        id: 0,
        hour: 7,
        minute: 0,
        days: 0,
        tone: String::new(),
        enabled: true,
    };
    let result = match which {
        "dial" => calls::dial("100", None, &[]).map(|_| ()),
        "answer" => calls::answer(1),
        "reject" => calls::reject(1),
        "hangup" => calls::hangup(1),
        "send-dtmf" => calls::send_dtmf(1, "12"),
        "dial-voicemail" => calls::dial_voicemail(None),
        "set-status" => presence::set_status(PresenceStatus::Available, ""),
        "login" => hotdesk::login("100", "0000"),
        "logout" => hotdesk::logout(),
        "set-alarm" => alarms::set(&alarm).map(|_| ()),
        "remove-alarm" => alarms::remove(1),
        "dismiss" => alarms::dismiss(),
        "play" => audio::play(TONE, 0),
        other => return format!("unknown: {other}"),
    };
    format!("{which}={result:?}")
}

impl Checks {
    fn ask(&mut self, what: &str, arg: &str) -> String {
        match what {
            "timers" => until_refused(40, |_| timers::set(60_000, false).map(|_| ())),
            "observe" => until_refused(70, |n| events::observe(&format!("name.{n}"))),
            // A name that is already observed is not one more.
            "observe-again" => format!("{:?}", events::observe("name.0")),
            "presence" => until_refused(260, |n| presence::observe(&format!("{}", 1000 + n))),
            "presence-again" => {
                let again = presence::observe("1000");
                presence::stop_observing("1001");
                format!("{again:?} {:?}", presence::observe("2000"))
            }
            "selected" => {
                let list = ui::create(WidgetKind::TextList, ROOT, Rect::new(0, 40, 10, 10));
                let empty = ui::selected(list);
                ui::set_items(list, &[ListItem::new("a"), ListItem::new("b"), ListItem::new("c")]);
                let first = ui::selected(list);
                ui::set_selected(list, 2);
                let set = ui::selected(list);
                // Past the end: the last row.
                ui::set_selected(list, 9);
                let past = ui::selected(list);
                let text = ui::selected(self.out);
                ui::destroy(list);
                let dead = ui::selected(list);
                format!("empty={empty:?} first={first:?} set={set:?} past={past:?} text={text:?} dead={dead:?} window={:?}", ui::selected(ROOT))
            }
            // A group with an input in it, focused: `arg` says what is
            // then hidden.
            "hide" => {
                let group = ui::create(WidgetKind::Group, ROOT, Rect::new(0, 40, 50, 20));
                let input = ui::create(WidgetKind::Input, group, Rect::new(0, 0, 50, 20));
                let other = ui::create(WidgetKind::Input, ROOT, Rect::new(0, 60, 50, 20));
                ui::set_input_mode(input, InputMode::Numeric);
                ui::set_focus(input);
                match arg {
                    "group" => ui::set_visible(group, false),
                    "input" => ui::set_visible(input, false),
                    "other" => ui::set_visible(other, false),
                    _ => {}
                }
                format!("input={input}")
            }
            "times" => {
                // 2026-11-13 22:05:00 UTC.
                let t = 1_794_607_500;
                [TimeStyle::Time, TimeStyle::Date, TimeStyle::DateAndTime]
                    .map(|style| l10n::format_time(t, style))
                    .join("|")
            }
            "login" => format!("{:?}", hotdesk::login(arg, "0000")),
            "logout" => format!("{:?}", hotdesk::logout()),
            "set" => stored(storage::set(StorageLocation::Local, arg, b"v", None)),
            "get" => stored(storage::get(StorageLocation::Local, arg)),
            "panic" => panic!("{arg}"),
            "watch" => format!("{:?}", events::observe(arg)),
            "calls" => {
                calls::observe(true);
                "observing".to_string()
            }
            "draw" => {
                let text = ui::create(WidgetKind::Text, ROOT, Rect::new(0, 40, 10, 10));
                let c = canvas::create(ROOT, Rect::new(0, 50, 10, 10));
                let on_canvas = canvas::draw(c, &fill());
                let on_text = canvas::draw(text, &fill());
                ui::destroy(c);
                let on_dead = canvas::draw(c, &fill());
                let on_window = canvas::draw(ROOT, &fill());
                format!("canvas={on_canvas:?} text={on_text:?} dead={on_dead:?} window={on_window:?}")
            }
            "pictures" => {
                let good = ui::define_image("good", PIC);
                let bad = ui::define_image("bad", b"GIF89a, not a PNG");
                // The refused content does not take the place of a picture.
                let again = ui::define_image("good", b"nor is this");
                let image = ui::create(WidgetKind::Image, ROOT, Rect::new(0, 60, 10, 10));
                ui::set_image(image, &Image::Picture("good".into()));
                ui::set_image(image, &Image::Picture("bad".into()));
                format!("good={good:?} bad={bad:?} again={again:?}")
            }
            "show" => format!("{:?}", runtime::show_app(arg)),
            "send" => format!("{:?}", events::send(arg, "hello", "there")),
            "lamps" => {
                let names: Vec<String> = lamps::list()
                    .into_iter()
                    .map(|l| match l {
                        Lamp::Line(n) => format!("line{n}"),
                        Lamp::Blf(n) => format!("blf{n}"),
                        Lamp::Message => "message".into(),
                        Lamp::Mute => "mute".into(),
                        Lamp::Headset => "headset".into(),
                        Lamp::Speaker => "speaker".into(),
                    })
                    .collect();
                names.join(",")
            }
            // What the phone says of its screen and of itself.
            "metrics" => {
                let m = ui::get_metrics();
                format!(
                    "{:?} {}x{} line={} title={} row={} bar={} color={} keys={} back={} model={:?}",
                    m.skin,
                    m.width,
                    m.height,
                    m.line_height,
                    m.title_height,
                    m.row_height,
                    m.scroll_bar_width,
                    m.color,
                    m.keys,
                    m.back_width,
                    config::model()
                )
            }
            // A canvas that takes touches, a list that does not, and with
            // the argument "window" the window takes the rest.
            "touch" => {
                let c = canvas::create(ROOT, Rect::new(100, 200, 300, 300));
                ui::claim_touch(c, true);
                let list = ui::create(WidgetKind::TextList, ROOT, Rect::new(0, 600, 400, 300));
                ui::set_items(list, &[ListItem::new("one"), ListItem::new("two"), ListItem::new("three")]);
                ui::claim_touch(ROOT, arg == "window");
                format!("canvas={c} list={list}")
            }
            "long-key" => {
                let key = "k".repeat(arg.parse().unwrap_or(0));
                stored(storage::set(StorageLocation::Local, &key, b"v", None))
            }
            "play" => format!("{:?}", audio::play(TONE, 0)),
            "stop" => {
                audio::stop();
                "stopped".to_string()
            }
            "preview" => {
                alarms::preview((!arg.is_empty()).then_some(arg));
                "previewed".to_string()
            }
            "hangup" => format!("{:?}", calls::hangup(arg.parse().unwrap_or(0))),
            "answer" => format!("{:?}", calls::answer(arg.parse().unwrap_or(0))),
            "leave" => {
                runtime::background();
                "left".to_string()
            }
            // One function that waits for the phone's answer, or "all".
            "wait" if arg == "all" => WAITS.map(wait).join(" "),
            "wait" => wait(arg),
            // Waits without end.
            "wait-forever" => loop {
                let _ = wait(arg);
            },
            // A wait, then the app's code runs without end.
            "wait-then-spin" => {
                let _ = wait(arg);
                let mut x = 0u32;
                loop {
                    x = x.wrapping_add(1);
                    core::hint::black_box(x);
                }
            }
            other => format!("unknown: {other}"),
        }
    }
}

impl App for Checks {
    fn start(&mut self) {
        self.out = ui::create(WidgetKind::Text, ROOT, Rect::new(0, 0, 100, 20));
        self.seen = ui::create(WidgetKind::Text, ROOT, Rect::new(0, 20, 100, 20));
        // Stay loaded when off the screen.
        runtime::set_exit_after_background(false);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Message((what, arg)) => {
                let answer = self.ask(&what, &arg);
                ui::set_text(self.out, &answer);
                return;
            }
            Event::Call(c) => self.log.push(format!("call {} {}", c.id, state(c.state))),
            Event::Push(p) => self.log.push(format!(
                "push {} kind={:?} data={:?} method={} call={:?}",
                p.name,
                p.kind,
                p.data,
                match p.method {
                    PushMethod::Message => "MESSAGE",
                    PushMethod::Invite => "INVITE",
                    PushMethod::Info => "INFO",
                },
                p.call
            )),
            Event::PlaybackStopped(at) => self.log.push(format!("stopped {at}")),
            Event::HotdeskDone(result) => self.log.push(format!("hotdesk {result:?}")),
            Event::Touch((w, Touch::Down(p))) => self.log.push(format!("down {w} {},{}", p.x, p.y)),
            Event::Touch((w, Touch::Tap(p))) => self.log.push(format!("tap {w} {},{}", p.x, p.y)),
            Event::Touch((w, Touch::Drag((a, b)))) => self.log.push(format!("drag {w} {},{} {},{}", a.x, a.y, b.x, b.y)),
            Event::Touch((w, Touch::DragEnd((a, b)))) => self.log.push(format!("end {w} {},{} {},{}", a.x, a.y, b.x, b.y)),
            Event::Widget((w, WidgetEvent::Highlighted(row))) => self.log.push(format!("highlighted {w} {row}")),
            Event::Widget((w, WidgetEvent::Activated(row))) => self.log.push(format!("activated {w} {row}")),
            _ => return,
        }
        ui::set_text(self.seen, &self.log.join("; "));
    }
}

opendeskphone_app_sdk::export_app! {
    app: Checks,
    name: "checks",
    id: "test.checks",
    display_name: "Checks",
}
