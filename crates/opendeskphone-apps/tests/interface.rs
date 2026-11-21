//! What the app interface promises an app: refusals that are reported,
//! events that arrive once and with the right content, and one app's
//! doings that leave another app's alone.  The test guest `checks` does
//! what a test asks and shows what the phone answered.

mod common;

use common::{Rig, staged};
use opendeskphone_apps::testutil::FakeConfig;
use opendeskphone_state::{AccountId, Call, CallId, CallState, Command, Direction, EndCause, Event, Key, Model, Party};

fn rig() -> Rig {
    let mut r = Rig::new(FakeConfig::default());
    r.launch("checks");
    r
}

/// Asks the guest to do something; returns what the guest answered.
fn ask(r: &mut Rig, what: &str, arg: &str) -> String {
    r.store.emit(Event::App { app: "checks".into(), name: what.into(), payload: arg.into() });
    r.settle();
    r.ui.texts("checks")[0].clone()
}

/// The calls and pushes the guest was told of.
fn seen(r: &Rig) -> String {
    r.ui.texts("checks")[1].clone()
}

/// The commands the phone got, once the threads that answer them have
/// caught up.
fn commands(r: &Rig) -> Vec<Command> {
    std::thread::sleep(std::time::Duration::from_millis(30));
    r.commands()
}

fn hangups(r: &Rig) -> Vec<u32> {
    commands(r).iter().filter_map(|c| if let Command::Hangup { call } = c { Some(call.0) } else { None }).collect()
}

fn call(id: u32, hidden: bool) -> Call {
    let mut call = Call::new(CallId(id), AccountId(0), Direction::Incoming, Party::default());
    call.hidden = hidden;
    call
}

#[test]
fn a_limit_that_is_reached_is_reported() {
    let mut r = rig();
    let timers = ask(&mut r, "timers", "");
    assert!(timers.starts_with("32 then ") && timers.contains("Failed("), "{timers}");
    let observed = ask(&mut r, "observe", "");
    assert!(observed.starts_with("64 then ") && observed.contains("Failed("), "{observed}");
    assert_eq!(ask(&mut r, "observe-again", ""), "Ok(())");
    let watched = ask(&mut r, "presence", "");
    assert!(watched.starts_with("256 then ") && watched.contains("Failed("), "{watched}");
    // A target that is already observed is not one more, and one that is
    // no longer observed makes room.
    assert_eq!(ask(&mut r, "presence-again", ""), "Ok(()) Ok(())");
}

#[test]
fn the_selected_row_is_none_where_there_is_no_row() {
    let mut r = rig();
    assert_eq!(
        ask(&mut r, "selected", ""),
        "empty=None first=Some(0) set=Some(2) past=Some(2) text=None dead=None window=None"
    );
}

#[test]
fn hiding_a_widget_drops_the_focus_inside() {
    for (hidden, keeps) in [("none", true), ("other", true), ("input", false), ("group", false)] {
        let mut r = rig();
        let answer = ask(&mut r, "hide", hidden);
        let input: u32 = answer.strip_prefix("input=").unwrap().parse().unwrap();
        assert_eq!(r.ui.focused("checks"), keeps.then_some(input), "hiding {hidden}");
    }
}

#[test]
fn the_styles_of_a_time_have_names() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "times", ""), "10:05 PM|Nov 13, 2026|Nov 13, 2026 10:05 PM");
}

#[test]
fn what_a_handler_changes_is_held_until_the_handler_returns() {
    let mut r = rig();
    let (held, before) = r.ui.holds("checks");
    assert!(!held);
    // One event, one handler: the window's changes are held once, and
    // let go when the handler has returned.
    r.store.emit(Event::App { app: "checks".into(), name: "selected".into(), payload: String::new() });
    r.settle();
    assert_eq!(r.ui.holds("checks"), (false, before + 1));
}

#[test]
fn a_login_tells_the_app_that_asked_how_it_ended() {
    let mut r = rig();
    r.launch("probe");
    assert_eq!(ask(&mut r, "login", "200"), "Ok(())");
    assert!(commands(&r).iter().any(|c| matches!(c, Command::SelectUser { id, .. } if id == "200")));
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = Some(Err("Invalid password (-199)".into())));
    r.settle();
    assert_eq!(seen(&r), r#"hotdesk Err("Invalid password (-199)")"#);
    // Once, and only for a login that the app asked for.
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.assigned_user = "100".into());
    r.settle();
    assert_eq!(seen(&r), r#"hotdesk Err("Invalid password (-199)")"#);

    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = None);
    assert_eq!(ask(&mut r, "logout", ""), "Ok(())");
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = Some(Ok(())));
    r.settle();
    assert_eq!(seen(&r), r#"hotdesk Err("Invalid password (-199)"); hotdesk Ok(())"#);
}

#[test]
fn a_login_that_is_refused_at_once_has_no_result_later() {
    // No extension: the phone asks the PBX nothing, and no event follows.
    let mut r = rig();
    assert!(ask(&mut r, "login", "").starts_with("Err(Error::Invalid("));
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = Some(Ok(())));
    r.settle();
    assert_eq!(seen(&r), "");
}

#[test]
fn a_drawing_for_a_widget_that_is_no_canvas_is_refused() {
    let mut r = rig();
    assert_eq!(
        ask(&mut r, "draw", ""),
        r#"canvas=Ok(()) text=Err("not a canvas") dead=Err("not a canvas") window=Err("not a canvas")"#
    );
}

#[test]
fn content_that_is_no_picture_is_refused_when_defined() {
    let mut r = rig();
    let answer = ask(&mut r, "pictures", "");
    assert!(answer.starts_with("good=Ok(()) bad=Err(Error::Invalid("), "{answer}");
    assert!(answer.contains(" again=Err(Error::Invalid("), "{answer}");
    // The window has the one picture, as first defined.
    let pic = std::fs::metadata(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/guests/probe/data/pic.png")).unwrap();
    assert_eq!(r.ui.images("checks"), [("good".to_string(), pic.len() as usize)]);
}

#[test]
fn showing_an_app_the_phone_does_not_offer_fails() {
    let config = FakeConfig::default();
    config.disabled.lock().unwrap().push("status".into());
    let mut r = Rig::new(config);
    r.launch("checks");
    let missing = ask(&mut r, "show", "nosuch");
    assert!(missing.starts_with("Err(") && missing.contains("no app named"), "{missing}");
    let off = ask(&mut r, "show", "status");
    assert!(off.starts_with("Err(") && off.contains("switched off"), "{off}");
    assert_eq!(r.ui.top().as_deref(), Some("checks"));
    assert_eq!(ask(&mut r, "show", "calendar"), "Ok(())");
    assert_eq!(r.ui.top().as_deref(), Some("calendar"));
}

#[test]
fn a_message_for_an_app_that_does_not_run_fails() {
    let mut r = rig();
    let stopped = ask(&mut r, "send", "probe");
    assert!(stopped.starts_with("Err(") && stopped.contains("not running"), "{stopped}");
    let missing = ask(&mut r, "send", "nosuch");
    assert!(missing.starts_with("Err("), "{missing}");
    r.launch("probe");
    assert_eq!(ask(&mut r, "send", "probe"), "Ok(())");
    // An app that runs may write to itself.
    assert_eq!(ask(&mut r, "send", "checks"), "unknown: hello");
}

#[test]
fn the_lamps_are_those_the_model_has() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "lamps", ""), "line1,line2,message,mute,headset,speaker");
    let mut r = Rig::with_root(staged(), FakeConfig::default(), Model::D65);
    r.launch("checks");
    assert_eq!(ask(&mut r, "lamps", ""), "line1,line2,line3,line4,line5,line6,message,mute,headset,speaker");
}

#[test]
fn a_storage_key_is_at_most_64_bytes() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "long-key", "64"), format!("stored {}", "k".repeat(64)));
    let answer = ask(&mut r, "long-key", "65");
    assert!(answer.starts_with("refused: ") && answer.contains("64 bytes"), "{answer}");
    // Bytes: 33 two-byte letters are 66.
    let answer = ask(&mut r, "set", &"\u{e9}".repeat(33));
    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(ask(&mut r, "set", &"\u{e9}".repeat(32)).starts_with("stored "));
    // Refused before the file system is touched: no file, and no file
    // that the phone made on the way to one.
    let store = r.nvdata.join("appdata/checks");
    let names: Vec<String> =
        std::fs::read_dir(&store).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let answer = ask(&mut r, "get", &"k".repeat(65));
    assert!(answer.starts_with("refused: "), "{answer}");
    assert_eq!(ask(&mut r, "get", &"k".repeat(64)), "entry Some(\"v\")");
}

#[cfg(unix)]
#[test]
fn a_key_whose_file_is_a_symbolic_link_fails_with_the_reason() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "set", "plain"), "stored plain");
    let store = r.nvdata.join("appdata/checks");
    std::os::unix::fs::symlink(store.join("plain"), store.join("link")).unwrap();
    let answer = ask(&mut r, "set", "link");
    assert!(answer.starts_with("unreachable: ") && answer.contains("symbolic link"), "{answer}");
    let answer = ask(&mut r, "get", "link");
    assert!(answer.starts_with("unreachable: ") && answer.contains("symbolic link"), "{answer}");
}

#[test]
fn a_call_change_arrives_once() {
    let mut r = rig();
    ask(&mut r, "calls", "");
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call(9, false)));
    r.settle();
    assert_eq!(seen(&r), "call 9 ringing");
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls[0].state = CallState::Active);
    r.settle();
    assert_eq!(seen(&r), "call 9 ringing; call 9 active");

    // What an app does not see of a call is no change for the app.
    r.store.update::<opendeskphone_state::Calls, _>(|c| {
        c.calls[0].muted = true;
        c.calls[0].srtp = true;
        c.calls[0].sip_call_id = "abc@example".into();
    });
    r.settle();
    assert_eq!(seen(&r), "call 9 ringing; call 9 active");

    // The end: the call's record says so, and the phone's event follows.
    let mut ended = None;
    r.store.update::<opendeskphone_state::Calls, _>(|c| {
        c.calls[0].state = CallState::Ended(EndCause::RemoteHangup);
        ended = Some(c.calls[0].clone());
    });
    r.store.emit(Event::CallEnded { call: ended.unwrap() });
    r.settle();
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.clear());
    r.settle();
    assert_eq!(seen(&r), "call 9 ringing; call 9 active; call 9 ended");
}

#[test]
fn a_push_from_the_pbx_keeps_its_payload_whatever_is_in_it() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "watch", "example.event"), "Ok(())");
    // A payload that looks like the description of a call is a payload.
    let payload = r#"{"method":"refresh","call":5,"data":"inner","type":"t"}"#;
    r.store.emit(Event::Push { name: "example.event".into(), body: payload.into() });
    r.settle();
    assert_eq!(seen(&r), format!("push example.event kind=\"\" data={payload:?} method=MESSAGE call=None"));

    // A push that arrives with a call says so.
    r.store.emit(Event::CallPush {
        name: "example.event".into(),
        kind: "example.kind".into(),
        data: "with a call".into(),
        method: "INFO".into(),
        call: CallId(5),
    });
    r.settle();
    assert!(
        seen(&r).ends_with("; push example.event kind=\"example.kind\" data=\"with a call\" method=INFO call=Some(5)"),
        "{}",
        seen(&r)
    );
}

fn hidden_call_arrives(r: &mut Rig, id: u32) {
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call(id, true)));
    r.store.emit(Event::CallPush {
        name: "digium.incomingCall.example".into(),
        kind: String::new(),
        data: String::new(),
        method: "INVITE".into(),
        call: CallId(id),
    });
    r.settle();
}

#[test]
fn a_hidden_call_ends_with_the_app_that_got_it() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "watch", "digium.incomingCall.example"), "Ok(())");
    // The app is off the screen when its call arrives.
    r.key("checks", Key::Cancel);
    assert!(r.mgr.is_running("checks"));
    assert_eq!(r.ui.top(), None);
    hidden_call_arrives(&mut r, 21);
    assert!(seen(&r).contains("call=Some(21)"), "{}", seen(&r));

    // Another app comes and goes: the call is not that app's to end.
    r.launch("calendar");
    r.key("calendar", Key::Cancel);
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running("calendar"));
    assert_eq!(hangups(&r), [] as [u32; 0]);

    // The app that got the call shows itself, and leaves: the call ends.
    r.launch("checks");
    assert_eq!(hangups(&r), [] as [u32; 0]);
    ask(&mut r, "leave", "");
    assert_eq!(r.ui.top(), None);
    assert_eq!(hangups(&r), [21]);
}

#[test]
fn a_hidden_call_ends_when_its_app_stops() {
    let mut r = rig();
    assert_eq!(ask(&mut r, "watch", "digium.incomingCall.example"), "Ok(())");
    r.key("checks", Key::Cancel);
    hidden_call_arrives(&mut r, 22);
    r.mgr.close("checks");
    assert_eq!(hangups(&r), [22]);
}

#[test]
fn a_hidden_call_no_app_got_ends_when_the_app_on_screen_leaves() {
    let mut r = rig();
    // No app observes the event of this call.
    hidden_call_arrives(&mut r, 23);
    assert_eq!(hangups(&r), [] as [u32; 0]);
    r.key("checks", Key::Cancel);
    assert_eq!(hangups(&r), [23]);
}

#[test]
fn an_app_stops_only_the_sound_the_app_started() {
    use opendeskphone_state::{Domain, Reply};
    use std::sync::{Arc, Mutex};
    let mut r = rig();
    let audio = Arc::new(Mutex::new(Vec::new()));
    common::answer_commands(&r.store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
    // What the audio side was asked since the last look, by kind.
    let asked = |audio: &Arc<Mutex<Vec<Command>>>| -> Vec<&'static str> {
        std::thread::sleep(std::time::Duration::from_millis(30));
        audio
            .lock()
            .unwrap()
            .drain(..)
            .map(|c| match c {
                Command::PlayData { .. } => "play",
                Command::PlayAlarmTone { .. } => "preview",
                Command::StopSound => "stop",
                _ => "other",
            })
            .collect()
    };

    // The app plays nothing: whatever sounds (another app's sound, a
    // ringtone preview of the settings) is not the app's to stop.
    ask(&mut r, "stop", "");
    ask(&mut r, "preview", "");
    assert_eq!(asked(&audio), [] as [&str; 0]);

    // The app's sound is the app's to stop.
    assert_eq!(ask(&mut r, "play", ""), "Ok(())");
    ask(&mut r, "stop", "");
    assert_eq!(asked(&audio), ["play", "stop"]);
    // The sound is over (here: another sound took its place); stopping
    // again would stop that other sound.
    r.store.emit(Event::PlaybackStopped { app: "checks".into(), position_secs: 1 });
    r.settle();
    assert_eq!(seen(&r), "stopped 1");
    ask(&mut r, "stop", "");
    assert_eq!(asked(&audio), [] as [&str; 0]);

    // The preview the app started is the app's to stop, once.
    ask(&mut r, "preview", "Buzzer");
    ask(&mut r, "preview", "");
    ask(&mut r, "preview", "");
    assert_eq!(asked(&audio), ["preview", "stop"]);
}

/// The guest on a phone of this model.
fn rig_on(model: Model) -> Rig {
    let mut r = Rig::with_root(staged(), FakeConfig::default(), model);
    r.launch("checks");
    r
}

#[test]
fn the_metrics_tell_a_touchscreen_from_a_phone_with_keys() {
    // The D80: the touch skin, no keys, and a back button at the corner.
    assert_eq!(
        ask(&mut rig_on(Model::D80), "metrics", ""),
        "Skin::Touch 800x1160 line=44 title=96 row=96 bar=12 color=true keys=false back=96 model=PhoneModel::D80"
    );
    // The others are as they were, and have keys.
    assert_eq!(
        ask(&mut rig_on(Model::D60), "metrics", ""),
        "Skin::Color 480x243 line=25 title=40 row=32 bar=8 color=true keys=true back=0 model=PhoneModel::D60"
    );
    assert_eq!(
        ask(&mut rig_on(Model::D70), "metrics", ""),
        "Skin::LargeGray 320x140 line=17 title=17 row=17 bar=4 color=false keys=true back=0 model=PhoneModel::D70"
    );
    assert_eq!(
        ask(&mut rig_on(Model::D40), "metrics", ""),
        "Skin::SmallGray 240x102 line=15 title=15 row=15 bar=4 color=false keys=true back=0 model=PhoneModel::D40"
    );
}

#[test]
fn a_touch_comes_to_the_widget_that_takes_touches() {
    let mut r = rig_on(Model::D80);
    let answer = ask(&mut r, "touch", "");
    let (canvas, list) = answer.strip_prefix("canvas=").unwrap().split_once(" list=").unwrap();
    assert!(r.ui.takes_touches("checks", canvas.parse().unwrap()));
    assert!(!r.ui.takes_touches("checks", 0));
    // On the canvas: the place is counted from the canvas's corner.
    r.ui.tap("checks", 110, 220);
    // Beside it, nothing takes the touch.
    r.ui.tap("checks", 50, 220);
    // A drag, which leaves the canvas, and its end.
    r.ui.drag("checks", (150, 250), (450, 150));
    // The list has the built-in behavior: its second row.
    r.ui.tap("checks", 20, 600 + 96 + 10);
    r.settle();
    assert_eq!(
        seen(&r),
        format!(
            "down {canvas} 10,20; tap {canvas} 10,20; \
             down {canvas} 50,50; drag {canvas} 50,50 350,-50; end {canvas} 50,50 350,-50; \
             highlighted {list} 1; activated {list} 1"
        )
    );

    // With the window taking touches, the rest of the window is the
    // app's too, with places of the window; the canvas is still its own.
    let mut r = rig_on(Model::D80);
    let answer = ask(&mut r, "touch", "window");
    let canvas = answer.strip_prefix("canvas=").unwrap().split_once(' ').unwrap().0;
    assert!(r.ui.takes_touches("checks", 0));
    r.ui.tap("checks", 50, 220);
    r.ui.tap("checks", 399, 499);
    r.settle();
    assert_eq!(seen(&r), format!("down 0 50,220; tap 0 50,220; down {canvas} 299,299; tap {canvas} 299,299"));
}
