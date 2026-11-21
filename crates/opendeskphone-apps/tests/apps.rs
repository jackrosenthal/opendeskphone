//! The built-in apps under the real runtime, with a fake screen and a
//! scripted PBX.

mod common;

use common::Rig;
use opendeskphone_apps::testutil::FakeConfig;
use opendeskphone_state::{
    AccountId, Call, CallId, CallState, Command, Direction, EndCause, Event, Key, Party,
};
use serde_json::{Value, json};

fn rig() -> Rig {
    Rig::new(FakeConfig::dasp())
}

#[test]
fn status_lists_options_and_sets_one() {
    let mut r = rig();
    r.pbx.on("switchvox.users.presence.options.getList", |_| {
        Ok(json!({"presence_options": {"presence_option": [
            {"id": "1", "presence": "available", "sub_presence": "", "owner": "system", "active": "1"},
            {"id": "2", "presence": "away", "sub_presence": "Lunch", "owner": "system", "active": "0"},
            {"id": "3", "presence": "dnd", "sub_presence": "", "owner": "system", "active": "0"},
        ]}}))
    });
    r.pbx.on("switchvox.users.presence.getInfo", |_| {
        Ok(json!({"presence": {"id": "1", "presence": "available", "sub_presence": "", "message": ""}}))
    });
    r.pbx.on("switchvox.users.presence.update", |_| {
        Ok(json!({"success": {}}))
    });
    r.launch("status");
    assert_eq!(r.ui.top().as_deref(), Some("status"));
    assert_eq!(
        r.ui.items("status"),
        vec![
            ("Available".to_string(), "*".to_string()),
            ("Away - Lunch".to_string(), String::new()),
            ("Do Not Disturb".to_string(), String::new()),
        ]
    );
    assert_eq!(r.ui.softkeys("status"), ["Select", "Message", "Exit"]);
    // The host added the account and the credentials stayed out of the app.
    assert_eq!(
        r.pbx.last("switchvox.users.presence.getInfo").unwrap()["account_id"],
        "100"
    );
    let raw = &r.pbx.raw_requests()[0];
    assert_eq!(
        (raw.url.as_str(), raw.username.as_str()),
        ("dasp://pbx.example/json", "100")
    );

    r.key("status", Key::Down);
    r.key("status", Key::Select);
    let sent = r.pbx.last("switchvox.users.presence.update").unwrap();
    assert_eq!(sent["presence_option_id"], "2");
    // The phone's own presence follows, and the app leaves the screen.
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::SetPresence { status, .. }
        if *status == opendeskphone_state::PresenceStatus::Away))
    );
    assert_eq!(r.ui.top(), None);
    assert!(
        !r.mgr.is_running("status"),
        "a foreground app exits when it leaves the screen"
    );
}

#[test]
fn status_message_screen_and_error() {
    let mut r = rig();
    // No handlers: the PBX answers with an error.
    r.launch("status");
    assert!(r.ui.shows("status", "Unknown method"));
    assert_eq!(r.ui.softkeys("status"), ["Retry", "Exit"]);
    r.key("status", Key::Cancel);
    assert_eq!(r.ui.top(), None);
}

#[test]
fn parking_lists_and_retrieves() {
    let mut r = rig();
    r.pbx.on("switchvox.users.parkingLots.getList", |_| {
        Ok(json!({"parking_lots": {"parking_lot": [{
        "account_id": "100", "parking_lot_start": "701", "parking_lot_stop": "720",
        "parked_calls": {"parked_call": [
            {"from_caller_id_name": "Alice", "from_caller_id_number": "101",
             "duration": "75", "lotnum": "702", "current_call_id": "PJSIP/101-1"},
            {"from_caller_id_name": "", "from_caller_id_number": "5551234",
             "duration": "5", "lotnum": "701", "current_call_id": "PJSIP/t-2"},
        ]}}]}}))
    });
    r.launch("parking");
    assert_eq!(
        r.ui.items("parking"),
        vec![
            ("701  5551234".to_string(), "0:05".to_string()),
            ("702  Alice".to_string(), "1:15".to_string()),
        ]
    );
    // A park push refreshes the list.
    let before = r.pbx.count("switchvox.users.parkingLots.getList");
    r.store.emit(Event::Push {
        name: "digium.parkingLot.parked".into(),
        body: r#"{"lotnumber":"703"}"#.into(),
    });
    r.settle();
    assert_eq!(
        r.pbx.count("switchvox.users.parkingLots.getList"),
        before + 1
    );

    r.key("parking", Key::Down);
    r.softkey("parking", "Retrieve");
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Dial { target, .. } if target == "702"))
    );
    assert_eq!(r.ui.top(), None);
}

#[test]
fn queues_login_pause_and_details() {
    let mut r = rig();
    let state = std::sync::Arc::new(std::sync::Mutex::new(("logged_out".to_string(), false)));
    let s = state.clone();
    r.pbx.on("switchvox.users.callQueues.getList", move |_| {
        let (status, paused) = s.lock().unwrap().clone();
        let mut sales = json!({"name": "Sales", "account_id": "600", "loggedin_status": status,
            "total_members": "4", "members_logged_in": "2", "callers_waiting": "1"});
        if paused {
            sales["paused_time"] = json!("1");
        }
        Ok(json!({"callQueues": {"callQueue": [sales,
            {"name": "Support", "account_id": "601", "loggedin_status": "permanent"}]}}))
    });
    let s = state.clone();
    r.pbx.on("switchvox.users.callQueues.login", move |_| {
        s.lock().unwrap().0 = "logged_in".into();
        Ok(json!({"success": {}}))
    });
    let s = state.clone();
    r.pbx.on("switchvox.users.callQueues.pause", move |_| {
        s.lock().unwrap().1 = true;
        Ok(json!({"success": {}}))
    });
    r.pbx.on("switchvox.users.callQueues.getTodaysStatus", |_| {
        Ok(
            json!({"callQueue": {"overview": {"total_members": "4", "logged_in_count": "2",
            "members_on_call": "1", "callers_waiting": "3", "total_calls": "40",
            "completed_calls_today": "35", "abandoned_calls": "5"}}}),
        )
    });
    r.launch("queues");
    assert_eq!(
        r.ui.items("queues"),
        vec![
            ("Sales".to_string(), "Out".to_string()),
            ("Support".to_string(), "Always".to_string())
        ]
    );
    assert_eq!(r.ui.softkeys("queues"), ["Log In", "Details", "Exit"]);

    r.softkey("queues", "Log In");
    assert_eq!(
        r.pbx.last("switchvox.users.callQueues.login").unwrap()["call_queue_account_id"],
        "600"
    );
    assert_eq!(r.ui.items("queues")[0].1, "In");
    assert_eq!(
        r.ui.softkeys("queues"),
        ["Log Out", "Pause", "Details", "Exit"]
    );

    r.softkey("queues", "Pause");
    r.ui.type_text("queues", "Lunch");
    r.settle();
    r.softkey("queues", "Pause");
    let p = r.pbx.last("switchvox.users.callQueues.pause").unwrap();
    assert_eq!(
        (p["call_queue_account_id"].as_str(), p["reason"].as_str()),
        (Some("600"), Some("Lunch"))
    );
    assert_eq!(r.ui.items("queues")[0].1, "Paused");

    r.softkey("queues", "Details");
    assert!(r.ui.shows("queues", "Waiting: 3"));
    assert!(r.ui.shows("queues", "Abandoned: 5"));
    r.key("queues", Key::Cancel);
    assert_eq!(r.ui.items("queues").len(), 2);

    // Queues keeps running in the background, as the stock app does.
    r.key("queues", Key::Cancel);
    assert_eq!(r.ui.top(), None);
    assert!(r.mgr.is_running("queues"));
}

#[test]
fn hotdesking_logs_in_and_out() {
    let mut r = rig();
    r.launch("hotdesking");
    assert_eq!(r.ui.softkeys("hotdesking"), ["Log In", "Cancel"]);
    r.ui.type_text("hotdesking", "101");
    r.settle();
    r.key("hotdesking", Key::Select); // moves to the PIN field
    r.ui.type_text("hotdesking", "4321");
    r.settle();
    r.softkey("hotdesking", "Log In");
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::SelectUser { id, pin } if id == "101" && pin == "4321"))
    );
    // The app stays on screen until the phone says how the login ended.
    assert_eq!(r.ui.top().as_deref(), Some("hotdesking"));
    assert!(r.ui.shows("hotdesking", "Logging in 101..."));
    // A change of the provisioning state that is not the end of the login
    // is not taken for it.
    r.store
        .update::<opendeskphone_state::Provisioning, _>(|p| p.phase = opendeskphone_state::ProvisionPhase::Handshaking);
    r.settle();
    assert!(r.ui.shows("hotdesking", "Logging in 101..."));

    // The PBX refuses the PIN: the app says so, with the PBX's reason.
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| {
        p.selection = Some(Err("Invalid password (-199)".into()));
    });
    r.settle();
    assert_eq!(r.ui.top().as_deref(), Some("hotdesking"));
    assert!(r.ui.shows("hotdesking", "Invalid password (-199)"));
    assert_eq!(r.ui.softkeys("hotdesking"), ["Back"]);
    // The result is told once: another change of the state brings nothing.
    r.softkey("hotdesking", "Back");
    r.store
        .update::<opendeskphone_state::Provisioning, _>(|p| p.phase = opendeskphone_state::ProvisionPhase::Provisioned);
    r.settle();
    assert_eq!(r.ui.softkeys("hotdesking"), ["Log In", "Cancel"]);

    // Again, and the PBX takes the user.
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = None);
    r.ui.type_text("hotdesking", "101");
    r.settle();
    r.key("hotdesking", Key::Select);
    r.ui.type_text("hotdesking", "1234");
    r.settle();
    r.softkey("hotdesking", "Log In");
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| {
        p.assigned_user = "101".into();
        p.selection = Some(Ok(()));
    });
    r.settle();
    assert!(r.ui.shows("hotdesking", "Logged in as 101"));

    r.store.update::<opendeskphone_state::Provisioning, _>(|p| p.selection = None);
    r.softkey("hotdesking", "Log Out");
    r.softkey("hotdesking", "Yes");
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::SelectUser { id, .. } if id.is_empty()))
    );
    assert!(r.ui.shows("hotdesking", "Logging out..."));
    r.store.update::<opendeskphone_state::Provisioning, _>(|p| {
        p.assigned_user.clear();
        p.selection = Some(Ok(()));
    });
    r.settle();
    assert_eq!(r.ui.softkeys("hotdesking"), ["Log In", "Cancel"]);
}

const CONTACTS: &str = r#"<contacts group_name="Internal Extensions" editable="0" >
<contact first_name="Alice" last_name="Zeta" contact_type="sip" subscribe_to="auto_hint_101">
<numbers><number dial="101" label="Extension" primary="1" /><number dial="5550101" label="Mobile" /></numbers></contact>
<contact first_name="Bob" last_name="Young" contact_type="sip" subscribe_to="auto_hint_102">
<numbers><number dial="102" label="Extension" primary="1" /></numbers></contact>
</contacts>"#;

fn contacts_config(extra_group: bool) -> FakeConfig {
    let mut c = FakeConfig::dasp();
    c.contact_lists.push(("0".into(), CONTACTS.into()));
    if extra_group {
        c.contact_lists.push((
            "1".into(),
            r#"<contacts group_name="Vendors"><contact first_name="Carol" last_name="">
               <numbers><number dial="918005550199" label="Office" primary="1"/></numbers></contact></contacts>"#
                .into(),
        ));
    }
    // The phone's merged copy must not make everything show twice.
    c.contact_lists.push((
        "serialized_contacts.xml".into(),
        format!("<merged_contacts>{CONTACTS}</merged_contacts>"),
    ));
    c
}

#[test]
fn contacts_browse_search_and_dial() {
    let mut r = Rig::new(contacts_config(false));
    r.store.update::<opendeskphone_state::Presence, _>(|p| {
        p.entries.push(opendeskphone_state::BlfEntry {
            id: "c1".into(),
            number: "101".into(),
            state: opendeskphone_state::BlfState::InUse,
            ..Default::default()
        })
    });
    r.launch("contacts");
    assert_eq!(
        r.ui.items("contacts"),
        vec![
            ("Alice Zeta".to_string(), "On call".to_string()),
            ("Bob Young".to_string(), String::new())
        ]
    );
    // Presence changes update the list in place.
    r.store
        .update::<opendeskphone_state::Presence, _>(|p| p.entries[0].state = opendeskphone_state::BlfState::Idle);
    r.settle();
    assert_eq!(r.ui.items("contacts")[0].1, "Available");

    r.softkey("contacts", "Search");
    r.ui.type_text("contacts", "bo");
    r.settle();
    assert_eq!(
        r.ui.items("contacts"),
        vec![("Bob Young".to_string(), String::new())]
    );
    // Digits match names by keypad letters (9 = w, x, y, z).
    r.ui.type_text("contacts", "93");
    r.settle();
    assert_eq!(r.ui.items("contacts")[0].0, "Alice Zeta");
    r.softkey("contacts", "Details");
    assert_eq!(
        r.ui.items("contacts"),
        vec![
            ("Extension".to_string(), "101".to_string()),
            ("Mobile".to_string(), "5550101".to_string())
        ]
    );
    r.key("contacts", Key::Down);
    r.softkey("contacts", "Dial");
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Dial { target, .. } if target == "5550101"))
    );
    assert_eq!(r.ui.top(), None);
}

#[test]
fn contacts_groups() {
    let mut r = Rig::new(contacts_config(true));
    r.launch("contacts");
    assert_eq!(
        r.ui.items("contacts"),
        vec![
            ("Internal Extensions".to_string(), "2".to_string()),
            ("Vendors".to_string(), "1".to_string())
        ]
    );
    r.key("contacts", Key::Down);
    r.key("contacts", Key::Select);
    assert_eq!(
        r.ui.items("contacts"),
        vec![("Carol".to_string(), String::new())]
    );
    r.key("contacts", Key::Select);
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Dial { target, .. } if target == "918005550199"))
    );
}

fn voicemail_pbx(r: &Rig) {
    r.pbx.on("switchvox.users.voicemail.getList", |p| {
        assert_eq!(p["sort_field"], "date");
        if p["folder"] != "INBOX" {
            return Ok(json!({"messages": {"folder": p["folder"], "message": []}}));
        }
        Ok(
            json!({"messages": {"folder": "INBOX", "page_number": "1", "total_pages": "1",
            "items_per_page": "30", "total_items": "2", "message": [
            {"id": "msg0001", "caller_id": "\"Alice\" <101>", "caller_id_number": "101",
             "caller_id_name": "Alice", "date": "1794607500", "duration": "65", "read": "0"},
            {"id": "msg0000", "caller_id": "5551234", "caller_id_number": "5551234",
             "caller_id_name": "", "date": "1794500000", "duration": "9", "read": "1"}]}}),
        )
    });
    r.pbx.on("switchvox.users.voicemail.getFolderList", |_| {
        Ok(json!({"folders": {"folder": [{"name": "INBOX", "count": "2"}, {"name": "Old", "count": "0"}]}}))
    });
    for m in ["play", "markRead", "markUnread", "remove", "move"] {
        r.pbx.on(&format!("switchvox.users.voicemail.{m}"), |_| {
            Ok(json!({"success": {}}))
        });
    }
}

#[test]
fn voicemail_list_play_and_controls() {
    let mut r = rig();
    voicemail_pbx(&r);
    r.launch("voicemail");
    let items = r.ui.items("voicemail");
    assert_eq!(items[0].0, "* Alice");
    assert_eq!(items[0].1, "Nov 13, 2026 10:05 PM");
    assert_eq!(items[1].0, "5551234");

    r.softkey("voicemail", "Play");
    let play = r.pbx.last("switchvox.users.voicemail.play").unwrap();
    assert_eq!(
        (play["folder"].as_str(), play["message_id"].as_str()),
        (Some("INBOX"), Some("msg0001"))
    );
    assert!(r.ui.shows("voicemail", "Connecting"));

    // The PBX rings the phone with the playback event; the app answers.
    let mut call = Call::new(
        CallId(9),
        AccountId(0),
        Direction::Incoming,
        Party::default(),
    );
    call.state = CallState::Ringing;
    r.store
        .update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call.clone()));
    r.store.emit(Event::Push {
        name: "digium.incomingCall.voicemail".into(),
        body: String::new(),
    });
    r.settle();
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Answer { call, .. } if call.0 == 9))
    );

    r.key("voicemail", Key::Star);
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Dtmf { call, digit: '*' } if call.0 == 9))
    );

    // Hanging up ends playback and marks the message read.
    call.state = CallState::Ended(EndCause::RemoteHangup);
    r.store
        .update::<opendeskphone_state::Calls, _>(|c| c.calls.clear());
    r.store.emit(Event::CallEnded { call });
    r.settle();
    let mark = r.pbx.last("switchvox.users.voicemail.markRead").unwrap();
    assert_eq!(mark["message_ids"], json!(["msg0001"]));
    assert_eq!(r.ui.items("voicemail").len(), 2, "back on the list");
}

#[test]
fn voicemail_rejects_playback_call_when_not_on_screen() {
    let mut r = rig();
    voicemail_pbx(&r);
    r.launch("voicemail");
    r.key("voicemail", Key::Cancel);
    assert!(r.mgr.is_running("voicemail"));
    let mut call = Call::new(
        CallId(11),
        AccountId(0),
        Direction::Incoming,
        Party::default(),
    );
    call.state = CallState::Ringing;
    r.store
        .update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call));
    r.store.emit(Event::CallPush {
        name: "digium.incomingCall.voicemail".into(),
        kind: String::new(),
        data: String::new(),
        method: "INVITE".into(),
        call: CallId(11),
    });
    r.settle();
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::Reject { call, .. } if call.0 == 11))
    );
}

#[test]
fn voicemail_delete_move_and_folders() {
    let mut r = rig();
    voicemail_pbx(&r);
    r.launch("voicemail");
    r.softkey("voicemail", "Delete");
    assert!(r.ui.shows("voicemail", "Delete the message from Alice?"));
    r.softkey("voicemail", "Yes");
    let rm = r.pbx.last("switchvox.users.voicemail.remove").unwrap();
    assert_eq!(
        (rm["folder"].as_str(), &rm["message_ids"]),
        (Some("INBOX"), &json!(["msg0001"]))
    );

    r.key("voicemail", Key::Down);
    r.softkey("voicemail", "Move");
    assert_eq!(
        r.ui.items("voicemail")[1],
        ("Old".to_string(), "0".to_string())
    );
    r.key("voicemail", Key::Down);
    r.softkey("voicemail", "Move");
    let mv = r.pbx.last("switchvox.users.voicemail.move").unwrap();
    assert_eq!(
        (
            mv["folder_from"].as_str(),
            mv["folder_to"].as_str(),
            &mv["message_ids"]
        ),
        (Some("INBOX"), Some("Old"), &json!(["msg0000"]))
    );

    r.softkey("voicemail", "Folders");
    r.key("voicemail", Key::Down);
    r.key("voicemail", Key::Select);
    assert!(r.ui.shows("voicemail", "No messages."));
    let last: Value = r.pbx.last("switchvox.users.voicemail.getList").unwrap();
    assert_eq!(last["folder"], "Old");
}

#[test]
fn apps_fit_every_skin() {
    // Each app must start and draw on the small gray and large gray
    // screens too (240x102 and 320x140 content areas).
    for model in [opendeskphone_state::Model::D40, opendeskphone_state::Model::D70] {
        let mut r = Rig::with_root(common::staged(), contacts_config(false), model);
        voicemail_pbx(&r);
        for app in [
            "contacts",
            "voicemail",
            "status",
            "parking",
            "queues",
            "hotdesking",
        ] {
            r.launch(app);
            assert!(
                r.ui.widget_count(app) > 0,
                "{app} drew nothing on {model:?}"
            );
            r.mgr.close(app);
        }
    }
}

/// 2026-11-14 01:00:00 UTC, which is the evening of November 13 seven
/// hours west.
const NOV_14_0100_UTC: u64 = 1_794_618_000;

fn calendar_rig(wall_time: u64) -> (Rig, std::sync::Arc<std::sync::atomic::AtomicU64>) {
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(wall_time));
    let config = FakeConfig {
        utc_offset: -7 * 3600,
        wall_time: Some(clock.clone()),
        ..Default::default()
    };
    (Rig::new(config), clock)
}

/// The calendar's title, and its 42 day cells (after the title and the
/// seven weekday names).
fn calendar_month(r: &Rig) -> (String, Vec<String>) {
    let texts = r.ui.texts("calendar");
    assert_eq!(texts.len(), 50, "{texts:?}");
    assert_eq!(texts[1..8], ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);
    (texts[0].clone(), texts[8..].to_vec())
}

/// The day cells of a month that starts on weekday `first` (0 is Sunday).
fn month_cells(first: usize, days: u32) -> Vec<String> {
    let mut cells = vec![String::new(); 42];
    for d in 1..=days {
        cells[first + d as usize - 1] = d.to_string();
    }
    cells
}

#[test]
fn calendar_shows_the_month_and_marks_today() {
    let (mut r, clock) = calendar_rig(NOV_14_0100_UTC);
    r.launch("calendar");
    assert_eq!(r.ui.softkeys("calendar"), ["Prev", "Today", "Next", "Exit"]);
    // Local time decides the date: it is still the 13th.
    assert_eq!(
        calendar_month(&r),
        ("November 2026".to_string(), month_cells(0, 30))
    );
    assert_eq!(r.ui.highlighted("calendar"), ["13"]);

    // Another month has no mark.
    r.key("calendar", Key::Right);
    assert_eq!(
        calendar_month(&r),
        ("December 2026".to_string(), month_cells(2, 31))
    );
    assert!(r.ui.highlighted("calendar").is_empty());
    r.key("calendar", Key::Down);
    assert_eq!(calendar_month(&r).0, "January 2027");
    r.softkey("calendar", "Today");
    assert_eq!(calendar_month(&r).0, "November 2026");
    assert_eq!(r.ui.highlighted("calendar"), ["13"]);

    // February, in a common year and in a leap year.
    for _ in 0..9 {
        r.key("calendar", Key::Left);
    }
    assert_eq!(
        calendar_month(&r),
        ("February 2026".to_string(), month_cells(0, 28))
    );
    r.key("calendar", Key::Up);
    assert_eq!(calendar_month(&r).0, "January 2026");
    r.softkey("calendar", "Prev");
    assert_eq!(calendar_month(&r).0, "December 2025");
    for _ in 0..26 {
        r.softkey("calendar", "Next");
    }
    assert_eq!(
        calendar_month(&r),
        ("February 2028".to_string(), month_cells(2, 29))
    );

    // Midnight passes while the current month is shown: the mark moves.
    r.softkey("calendar", "Today");
    clock.store(
        NOV_14_0100_UTC + 6 * 3600,
        std::sync::atomic::Ordering::Relaxed,
    );
    let wait = r.mgr.pump().expect("the date check is pending");
    std::thread::sleep(wait + std::time::Duration::from_millis(20));
    r.settle();
    assert_eq!(r.ui.highlighted("calendar"), ["14"]);

    r.key("calendar", Key::Cancel);
    assert_eq!(r.ui.top(), None);
}

#[test]
fn calendar_without_a_set_clock() {
    // The phones have no clock chip: a fresh one starts near 1970.
    let (mut r, clock) = calendar_rig(600);
    r.launch("calendar");
    assert!(r.ui.shows("calendar", "The date is not set."));
    assert_eq!(r.ui.softkeys("calendar"), ["Exit"]);
    r.key("calendar", Key::Right);
    assert!(r.ui.shows("calendar", "The date is not set."));

    // The clock is set while the app is on screen.
    clock.store(NOV_14_0100_UTC, std::sync::atomic::Ordering::Relaxed);
    let wait = r.mgr.pump().expect("the date check is pending");
    std::thread::sleep(wait + std::time::Duration::from_millis(20));
    r.settle();
    assert_eq!(calendar_month(&r).0, "November 2026");
    assert_eq!(r.ui.highlighted("calendar"), ["13"]);
    r.softkey("calendar", "Exit");
    assert_eq!(r.ui.top(), None);
}

#[test]
fn diagnostics_menu_tests_and_status_pages() {
    use opendeskphone_apps::DrawOp;
    use opendeskphone_state::{Domain, HealthStatus, LampColor, Level, Reply, Subsystem};
    const APP: &str = "diagnostics";
    let mut r = rig();
    common::answer_commands(&r.store, Domain::Audio, r.commands.clone(), |_| Ok(Reply::Done));
    r.launch(APP);
    let items: Vec<String> = r.ui.items(APP).into_iter().map(|i| i.0).collect();
    assert_eq!(
        items,
        ["Display test", "Key test", "Lamp test", "Tone test", "Subsystems", "Log"]
    );
    let menu = ["Select", "", "", "Back"];
    assert_eq!(r.ui.softkeys(APP), menu);
    assert!(!r.ui.captures(APP) && r.ui.lamps(APP).is_empty());

    // The key test is the app's: it takes every key and the hook, and
    // shows their names.  Cancel twice in a row leaves it, for the menu
    // at the same row.
    r.key(APP, Key::Down);
    r.key(APP, Key::Select);
    assert!(r.ui.captures(APP) && !r.ui.full_screen(APP));
    assert_eq!(r.ui.texts(APP), ["Key test", "Cancel twice to exit", ""]);
    for key in [Key::Line(1), Key::VolumeUp, Key::Cancel, Key::Soft(2), Key::Cancel] {
        r.key(APP, key);
    }
    r.ui.hook(APP, true);
    r.ui.hook(APP, false);
    r.settle();
    assert_eq!(r.ui.texts(APP)[2], "line1 vup cancel sk2 cancel hook-up hook-down");
    // The newest 24 show.
    for _ in 0..12 {
        r.key(APP, Key::Digit(7));
        r.key(APP, Key::Mute);
    }
    assert_eq!(r.ui.texts(APP)[2], ["7 mute"; 12].join(" "));
    r.key(APP, Key::Cancel);
    assert!(r.ui.captures(APP));
    r.key(APP, Key::Cancel);
    assert!(!r.ui.captures(APP));
    assert_eq!(r.ui.softkeys(APP), menu);
    r.softkey(APP, "Select");
    assert_eq!(r.ui.texts(APP)[0], "Key test");
    r.key(APP, Key::Cancel);
    r.key(APP, Key::Cancel);
    assert!(r.mgr.is_running(APP));

    // The display test: six patterns on the whole display, the next one
    // at any key, and the menu after the last.  Right selects, as in the
    // phone's menus.
    r.key(APP, Key::Up);
    r.key(APP, Key::Right);
    let pattern = |r: &Rig| -> Vec<DrawOp> {
        let mut drawings = r.ui.drawings(APP);
        assert_eq!(drawings.len(), 1);
        let (bounds, ops) = drawings.remove(0);
        assert_eq!((bounds.width, bounds.height), (480, 272));
        ops
    };
    let fills = |ops: &[DrawOp]| ops.iter().filter(|o| matches!(o, DrawOp::Fill(_))).count();
    assert!(r.ui.captures(APP) && r.ui.full_screen(APP));
    assert!(matches!(&pattern(&r)[..], [DrawOp::Fill((all, c))] if all.width == 480 && all.height == 272 && (c.r, c.g, c.b) == (255, 255, 255)));
    r.key(APP, Key::VolumeDown);
    assert!(matches!(&pattern(&r)[..], [DrawOp::Fill((_, c))] if (c.r, c.g, c.b) == (0, 0, 0)));
    r.key(APP, Key::Line(1));
    assert_eq!(fills(&pattern(&r)), 8, "eight color bars");
    r.key(APP, Key::Select);
    // Every other pixel of every other row, in bitmaps of at most 256
    // pixels each way.
    let ops = pattern(&r);
    let bitmaps: Vec<_> = ops
        .iter()
        .filter_map(|o| match o {
            DrawOp::Bitmap(b) => Some((b.x, b.y, b.width, b.height, b.bits[0], b.bits[b.width as usize / 8])),
            _ => None,
        })
        .collect();
    assert_eq!(
        bitmaps,
        [(0, 0, 256, 256, 0xAA, 0), (256, 0, 224, 256, 0xAA, 0), (0, 256, 256, 16, 0xAA, 0), (256, 256, 224, 16, 0xAA, 0)]
    );
    r.key(APP, Key::Digit(0));
    assert_eq!(fills(&pattern(&r)), 11, "two frames and a cross");
    r.key(APP, Key::Soft(4));
    let texts = pattern(&r).iter().filter(|o| matches!(o, DrawOp::Text(t) if t.text.starts_with("Open Desk Phone"))).count();
    assert_eq!(texts, 5, "a line in every font");
    r.ui.hook(APP, true);
    r.settle();
    assert!(r.ui.full_screen(APP), "the hook is not a key");
    r.key(APP, Key::Hold);
    assert!(!r.ui.captures(APP) && !r.ui.full_screen(APP));
    assert_eq!(r.ui.softkeys(APP), menu);
    // Cancel leaves it at any pattern; the row's number selects.
    r.key(APP, Key::Digit(1));
    r.key(APP, Key::Star);
    assert!(r.ui.full_screen(APP));
    r.key(APP, Key::Cancel);
    assert!(!r.ui.captures(APP) && !r.ui.full_screen(APP));

    // The lamp test: every lamp of the phone, a color every half second.
    r.key(APP, Key::Digit(3));
    assert_eq!(r.ui.texts(APP), ["Lamp test", "Cycling all lamps"]);
    assert_eq!(r.ui.softkeys(APP), ["", "", "", "Back"]);
    assert!(!r.ui.captures(APP));
    let names: Vec<String> = opendeskphone_apps::Lamp::for_model(opendeskphone_state::Model::D60).into_iter().map(|l| l.name()).collect();
    assert_eq!(names, ["line1", "line2", "message", "mute", "hdst", "spkr"]);
    let all = |color: LampColor| -> Vec<(String, LampColor)> { names.iter().map(|n| (n.clone(), color)).collect() };
    assert_eq!(r.ui.lamps(APP), all(LampColor::Red));
    for color in [LampColor::Green, LampColor::Amber, LampColor::Off, LampColor::Red] {
        let wait = r.mgr.pump().expect("the next color is pending");
        assert!(wait <= std::time::Duration::from_millis(500));
        std::thread::sleep(wait + std::time::Duration::from_millis(20));
        r.settle();
        assert_eq!(r.ui.lamps(APP), all(color));
    }
    r.key(APP, Key::Select);
    assert!(r.ui.lamps(APP).is_empty(), "the lamps are the phone's again");
    assert_eq!(r.ui.softkeys(APP), menu);
    assert_eq!(r.mgr.pump(), None, "and nothing is pending");
    r.softkey(APP, "Select");
    assert_eq!(r.ui.lamps(APP), all(LampColor::Red));
    r.softkey(APP, "Back");
    assert!(r.ui.lamps(APP).is_empty());

    // The tone test: two seconds of dial tone, which the app makes.
    r.key(APP, Key::Digit(4));
    let played: Vec<Command> = r.commands().into_iter().filter(|c| matches!(c, Command::PlayData { .. })).collect();
    let [Command::PlayData { sound, start_secs: 0, app }] = &played[..] else { panic!("{played:?}") };
    assert_eq!(app, APP);
    let samples: Vec<i16> = sound.0.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
    assert_eq!(samples.len(), 2 * 16_000);
    // 350 Hz and 440 Hz at -13 dBm0 each, and nothing at, e.g., 620 Hz.
    let level = |hz: f64| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in samples[..16_000].iter().enumerate() {
            let t = std::f64::consts::TAU * hz * i as f64 / 16_000.0;
            re += f64::from(*s) * t.cos();
            im += f64::from(*s) * t.sin();
        }
        (re * re + im * im).sqrt() / 8000.0
    };
    assert!((level(350.0) - 5110.0).abs() < 60.0 && (level(440.0) - 5110.0).abs() < 60.0);
    assert!(level(620.0) < 30.0);

    // The subsystems page follows the phone's state while it shows.
    r.store.set_health(Subsystem::Net, HealthStatus::Ok);
    r.store
        .set_health(Subsystem::Media, HealthStatus::Degraded("no handset".into()));
    r.key("diagnostics", Key::Digit(5));
    assert!(!r.ui.captures(APP));
    let texts = r.ui.texts("diagnostics");
    assert_eq!(texts[0], "Subsystems");
    assert_eq!(texts.len(), 1 + 2 * Subsystem::ALL.len());
    let pair = |texts: &[String], label: &str| -> String {
        let at = texts.iter().position(|t| t == label).expect(label);
        texts[at + 1].clone()
    };
    assert_eq!(pair(&texts, Subsystem::Net.label()), "ok");
    assert_eq!(pair(&texts, Subsystem::Media.label()), "degraded: no handset");
    assert_eq!(r.ui.softkeys("diagnostics"), ["", "", "", "Back"]);
    r.store
        .set_health(Subsystem::Net, HealthStatus::Failed("cable".into()));
    let wait = r.mgr.pump().expect("the refresh is pending");
    std::thread::sleep(wait + std::time::Duration::from_millis(20));
    r.settle();
    assert_eq!(
        pair(&r.ui.texts("diagnostics"), Subsystem::Net.label()),
        "failed: cable"
    );
    r.key("diagnostics", Key::Left);
    assert_eq!(r.ui.softkeys("diagnostics"), ["Select", "", "", "Back"]);

    // The log, newest first.
    r.store.log().push(Level::Info, "test::first", "one".into());
    r.store.log().push(Level::Warn, "test::second", "two".into());
    r.key("diagnostics", Key::Digit(6));
    let texts = r.ui.texts("diagnostics");
    assert_eq!(texts[0], "Log");
    assert_eq!(texts[1..3], ["test::second two", "test::first one"]);
    r.softkey("diagnostics", "Back");
    assert_eq!(r.ui.softkeys("diagnostics"), ["Select", "", "", "Back"]);

    // Back from the menu leaves the app, which then stops.
    r.softkey("diagnostics", "Back");
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running("diagnostics"));
}

// ---------------------------------------------------------------- touch
//
// The D80 has a touchscreen and no keys: the apps are used through their
// lists, their softkey buttons, and the back button (which is the key
// `Cancel`).  A touch on a row does what the row is for, and no softkey
// does what the back button does.

use opendeskphone_state::Model;

/// A D80.
fn touch_rig(config: FakeConfig) -> Rig {
    Rig::with_root(common::staged(), config, Model::D80)
}

/// A finger on row `row` of a list that begins under the title, in the
/// D80's window: the title and each row are 96 pixels high.
fn tap_list_row(r: &mut Rig, app: &str, row: i32) {
    r.ui.tap(app, 400, 96 + row * 96 + 48);
    r.settle();
}

/// The back button.
fn back(r: &mut Rig, app: &str) {
    r.key(app, Key::Cancel);
}

#[test]
fn status_by_touch() {
    let mut r = touch_rig(FakeConfig::dasp());
    r.pbx.on("switchvox.users.presence.options.getList", |_| {
        Ok(json!({"presence_options": {"presence_option": [
            {"id": "1", "presence": "available", "sub_presence": "", "owner": "system", "active": "1"},
            {"id": "2", "presence": "away", "sub_presence": "Lunch", "owner": "system", "active": "0"},
            {"id": "3", "presence": "dnd", "sub_presence": "", "owner": "system", "active": "0"},
        ]}}))
    });
    r.pbx.on("switchvox.users.presence.getInfo", |_| {
        Ok(json!({"presence": {"id": "1", "presence": "available", "sub_presence": "", "message": ""}}))
    });
    r.pbx.on("switchvox.users.presence.update", |_| Ok(json!({"success": {}})));
    r.launch("status");
    assert_eq!(r.ui.items("status").len(), 3);
    assert_eq!(r.ui.softkeys("status"), ["Message"]);
    // The message's screen, and the back button out of it.
    r.softkey("status", "Message");
    assert_eq!(r.ui.softkeys("status"), ["OK"]);
    r.ui.type_text("status", "Back at 1");
    r.settle();
    back(&mut r, "status");
    assert_eq!(r.ui.items("status").len(), 3, "the back button leaves the message for the list");
    // Touching a status sets it.
    tap_list_row(&mut r, "status", 2);
    assert_eq!(r.pbx.last("switchvox.users.presence.update").unwrap()["presence_option_id"], "3");
    assert_eq!(r.ui.top(), None);
}

#[test]
fn parking_by_touch() {
    let mut r = touch_rig(FakeConfig::dasp());
    r.pbx.on("switchvox.users.parkingLots.getList", |_| {
        Ok(json!({"parking_lots": {"parking_lot": [{
        "account_id": "100", "parking_lot_start": "701", "parking_lot_stop": "720",
        "parked_calls": {"parked_call": [
            {"from_caller_id_name": "Alice", "from_caller_id_number": "101",
             "duration": "75", "lotnum": "702", "current_call_id": "PJSIP/101-1"},
            {"from_caller_id_name": "", "from_caller_id_number": "5551234",
             "duration": "5", "lotnum": "701", "current_call_id": "PJSIP/t-2"},
        ]}}]}}))
    });
    r.launch("parking");
    assert_eq!(r.ui.items("parking").len(), 2);
    assert_eq!(r.ui.softkeys("parking"), ["Refresh"]);
    // Touching a parked call takes it.
    tap_list_row(&mut r, "parking", 1);
    assert!(r.commands().iter().any(|c| matches!(c, Command::Dial { target, .. } if target == "702")));
    assert_eq!(r.ui.top(), None);
}

#[test]
fn queues_by_touch() {
    let mut r = touch_rig(FakeConfig::dasp());
    r.pbx.on("switchvox.users.callQueues.getList", |_| {
        Ok(json!({"callQueues": {"callQueue": [
            {"name": "Sales", "account_id": "600", "loggedin_status": "permanent"},
            {"name": "Support", "account_id": "601", "loggedin_status": "logged_out"}]}}))
    });
    r.pbx.on("switchvox.users.callQueues.login", |_| Ok(json!({"success": {}})));
    r.pbx.on("switchvox.users.callQueues.getTodaysStatus", |_| {
        Ok(
            json!({"callQueue": {"overview": {"total_members": "4", "logged_in_count": "2",
            "members_on_call": "1", "callers_waiting": "3", "total_calls": "40",
            "completed_calls_today": "35", "abandoned_calls": "5"}}}),
        )
    });
    r.launch("queues");
    // The list has no softkeys: no queue is the highlighted one.
    assert_eq!(r.ui.softkeys("queues"), [] as [&str; 0]);
    // Touching a queue shows its numbers, with the softkeys for it.
    tap_list_row(&mut r, "queues", 1);
    assert!(r.ui.shows("queues", "Waiting: 3"));
    assert_eq!(
        r.pbx.last("switchvox.users.callQueues.getTodaysStatus").unwrap()["call_queue_account_id"],
        "601"
    );
    assert_eq!(r.ui.softkeys("queues"), ["Log In", "Refresh"]);
    r.softkey("queues", "Log In");
    assert_eq!(r.pbx.last("switchvox.users.callQueues.login").unwrap()["call_queue_account_id"], "601");
    assert_eq!(r.ui.items("queues").len(), 2, "the list again, as it now is");
    // The first queue, which the user is always in: it can be paused, and
    // the back button leaves the question for the queue's numbers.
    tap_list_row(&mut r, "queues", 0);
    assert_eq!(r.ui.softkeys("queues"), ["Pause", "Refresh"]);
    r.softkey("queues", "Pause");
    assert_eq!(r.ui.softkeys("queues"), ["Pause"]);
    back(&mut r, "queues");
    assert!(r.ui.shows("queues", "Waiting: 3"));
    assert_eq!(
        r.pbx.last("switchvox.users.callQueues.getTodaysStatus").unwrap()["call_queue_account_id"],
        "600"
    );
    back(&mut r, "queues");
    assert_eq!(r.ui.items("queues").len(), 2);
    back(&mut r, "queues");
    assert_eq!(r.ui.top(), None);
}

#[test]
fn hotdesking_by_touch() {
    let mut r = touch_rig(FakeConfig::dasp());
    r.launch("hotdesking");
    // The form's rows are a row's height, with space between, under the
    // title: the first input has the focus, and a touch gives it to the
    // other.
    let extension = r.ui.focused("hotdesking");
    r.ui.type_text("hotdesking", "101");
    r.ui.tap("hotdesking", 500, 96 + 8 + 96 + 16 + 48);
    r.settle();
    let pin = r.ui.focused("hotdesking");
    assert!(pin.is_some() && pin != extension, "the PIN's input is touched");
    r.ui.type_text("hotdesking", "4321");
    r.settle();
    // And back to the first, which is as high as a row.
    r.ui.tap("hotdesking", 500, 96 + 8 + 90);
    assert_eq!(r.ui.focused("hotdesking"), extension);
    // A label is not an input.
    r.ui.tap("hotdesking", 100, 96 + 8 + 96 + 16 + 48);
    assert_eq!(r.ui.focused("hotdesking"), extension);
    assert_eq!(r.ui.softkeys("hotdesking"), ["Log In"]);
    r.softkey("hotdesking", "Log In");
    assert!(
        r.commands()
            .iter()
            .any(|c| matches!(c, Command::SelectUser { id, pin } if id == "101" && pin == "4321"))
    );
    assert!(r.ui.shows("hotdesking", "Logging in 101..."));
}

#[test]
fn contacts_by_touch() {
    let mut r = touch_rig(contacts_config(true));
    r.launch("contacts");
    // The groups; touching one opens it.
    assert_eq!(r.ui.softkeys("contacts"), ["Search"]);
    tap_list_row(&mut r, "contacts", 0);
    assert_eq!(r.ui.items("contacts").len(), 2);
    // No softkey is for a highlighted row (a contact is touched), and
    // the back button is the way back to the groups.
    assert_eq!(r.ui.softkeys("contacts"), ["Search"]);
    // Touching a contact shows its numbers, and dials nothing yet.
    tap_list_row(&mut r, "contacts", 0);
    assert_eq!(
        r.ui.items("contacts"),
        vec![
            ("Extension".to_string(), "101".to_string()),
            ("Mobile".to_string(), "5550101".to_string())
        ]
    );
    assert_eq!(r.ui.softkeys("contacts"), [] as [&str; 0]);
    assert!(!r.commands().iter().any(|c| matches!(c, Command::Dial { .. })));
    // The back button, and the search: its list is under the input.
    back(&mut r, "contacts");
    r.softkey("contacts", "Search");
    assert_eq!(r.ui.softkeys("contacts"), [] as [&str; 0]);
    r.ui.type_text("contacts", "bo");
    r.settle();
    assert_eq!(r.ui.items("contacts"), vec![("Bob Young".to_string(), String::new())]);
    r.ui.tap("contacts", 400, 96 + 96 + 2 + 48);
    r.settle();
    assert_eq!(r.ui.items("contacts"), vec![("Extension".to_string(), "102".to_string())]);
    // Touching a number dials it.
    tap_list_row(&mut r, "contacts", 0);
    assert!(r.commands().iter().any(|c| matches!(c, Command::Dial { target, .. } if target == "102")));
    assert_eq!(r.ui.top(), None);
    // From the list of a group, the back button goes to the groups.
    r.launch("contacts");
    tap_list_row(&mut r, "contacts", 0);
    assert_eq!(r.ui.items("contacts").len(), 2);
    back(&mut r, "contacts");
    assert_eq!(r.ui.items("contacts")[0].0, "Internal Extensions");
}

#[test]
fn voicemail_by_touch() {
    let mut r = touch_rig(FakeConfig::dasp());
    voicemail_pbx(&r);
    r.launch("voicemail");
    // The list's softkeys are not for a message: none is highlighted.
    assert_eq!(r.ui.softkeys("voicemail"), ["Folders"]);
    // Touching a message plays it, on its own screen: what `*`, `0`, and
    // `#` do on the other phones are softkeys, and so is what else is done
    // with the message.
    tap_list_row(&mut r, "voicemail", 0);
    assert_eq!(r.pbx.last("switchvox.users.voicemail.play").unwrap()["message_id"], "msg0001");
    assert!(r.ui.shows("voicemail", "Connecting"));
    assert_eq!(r.ui.softkeys("voicemail"), ["Rewind", "Pause", "Skip", "Delete", "Call Back", "Move"]);
    let mut call = Call::new(CallId(9), AccountId(0), Direction::Incoming, Party::default());
    call.state = CallState::Ringing;
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call.clone()));
    r.store.emit(Event::Push { name: "digium.incomingCall.voicemail".into(), body: String::new() });
    r.settle();
    for (key, digit) in [("Rewind", '*'), ("Pause", '0'), ("Skip", '#')] {
        r.softkey("voicemail", key);
        assert!(
            r.commands().iter().any(|c| matches!(c, Command::Dtmf { call, digit: d } if call.0 == 9 && *d == digit)),
            "{key}"
        );
    }
    // The message plays to its end: it is marked read, and its screen
    // stays, with what is done with a message that does not play.
    call.state = CallState::Ended(EndCause::RemoteHangup);
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.clear());
    r.store.emit(Event::CallEnded { call });
    r.settle();
    assert_eq!(r.pbx.last("switchvox.users.voicemail.markRead").unwrap()["message_ids"], json!(["msg0001"]));
    assert!(r.ui.shows("voicemail", "Alice") && !r.ui.shows("voicemail", "Connecting"));
    assert_eq!(r.ui.softkeys("voicemail"), ["Play", "Delete", "Call Back", "Move", "Unread"]);
    // It is played again, and the back button stops it: the list.
    r.softkey("voicemail", "Play");
    assert_eq!(r.pbx.count("switchvox.users.voicemail.play"), 2);
    let mut call = Call::new(CallId(10), AccountId(0), Direction::Incoming, Party::default());
    call.state = CallState::Ringing;
    r.store.update::<opendeskphone_state::Calls, _>(|c| c.calls.push(call.clone()));
    r.store.emit(Event::Push { name: "digium.incomingCall.voicemail".into(), body: String::new() });
    r.settle();
    back(&mut r, "voicemail");
    assert!(r.commands().iter().any(|c| matches!(c, Command::Hangup { call } if call.0 == 10)));
    assert_eq!(r.ui.items("voicemail").len(), 2, "back on the list");

    // The other message: "Delete" stops it and asks; "No" leaves the
    // message on its screen, and "Yes" deletes it.
    tap_list_row(&mut r, "voicemail", 1);
    assert_eq!(r.pbx.last("switchvox.users.voicemail.play").unwrap()["message_id"], "msg0000");
    r.softkey("voicemail", "Delete");
    assert!(r.ui.shows("voicemail", "Delete the message from 5551234?"));
    r.softkey("voicemail", "No");
    assert_eq!(r.ui.softkeys("voicemail"), ["Play", "Delete", "Call Back", "Move", "Unread"]);
    // It is moved by touching a folder, which has no softkeys.
    r.softkey("voicemail", "Move");
    assert_eq!(r.ui.softkeys("voicemail"), [] as [&str; 0]);
    back(&mut r, "voicemail");
    assert_eq!(r.ui.softkeys("voicemail"), ["Play", "Delete", "Call Back", "Move", "Unread"]);
    r.softkey("voicemail", "Move");
    tap_list_row(&mut r, "voicemail", 1);
    let mv = r.pbx.last("switchvox.users.voicemail.move").unwrap();
    assert_eq!((mv["folder_to"].as_str(), &mv["message_ids"]), (Some("Old"), &json!(["msg0000"])));
    assert_eq!(r.ui.items("voicemail").len(), 2, "back on the list");
    tap_list_row(&mut r, "voicemail", 1);
    r.softkey("voicemail", "Delete");
    r.softkey("voicemail", "Yes");
    assert_eq!(r.pbx.last("switchvox.users.voicemail.remove").unwrap()["message_ids"], json!(["msg0000"]));
    // From a message's screen, the back button goes to the list.
    tap_list_row(&mut r, "voicemail", 1);
    r.softkey("voicemail", "Delete");
    r.softkey("voicemail", "No");
    back(&mut r, "voicemail");
    assert_eq!(r.ui.items("voicemail").len(), 2, "back on the list");

    // A folder is opened by touching it.
    r.softkey("voicemail", "Folders");
    assert_eq!(r.ui.softkeys("voicemail"), [] as [&str; 0]);
    tap_list_row(&mut r, "voicemail", 1);
    assert!(r.ui.shows("voicemail", "No messages."));
    assert_eq!(r.ui.softkeys("voicemail"), ["Folders", "Refresh"]);
    assert_eq!(r.pbx.last("switchvox.users.voicemail.getList").unwrap()["folder"], "Old");
}

#[test]
fn a_playing_message_has_one_softkey_on_a_phone_with_keys() {
    // Rewind, pause, and skip are keys there.
    let mut r = rig();
    voicemail_pbx(&r);
    r.launch("voicemail");
    r.softkey("voicemail", "Play");
    assert_eq!(r.ui.softkeys("voicemail"), ["Stop"]);
}

// ------------------------------------------- the on-screen keyboard (D80)

/// A simulated D80 with the phone's own UI, which an app draws on and a
/// finger touches.
struct TouchPhone {
    ui: opendeskphone_ui::Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: opendeskphone_apps::AppManager,
    commands: std::sync::Arc<std::sync::Mutex<Vec<Command>>>,
    _dir: common::TempDir,
}

impl TouchPhone {
    fn new() -> TouchPhone {
        TouchPhone::with(FakeConfig::dasp())
    }

    fn with(fake: FakeConfig) -> TouchPhone {
        use opendeskphone_apps::testutil::{FakePbx, FakeStore, services_with_store};
        use opendeskphone_state::{Domain, Phase, Reply, Store, System};
        let model = Model::D80;
        let dir = common::temp_dir("touch-phone");
        let mut hal = opendeskphone_hal::Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let config = opendeskphone_ui::UiConfig { backlight_timeout_secs: 0, ..Default::default() };
        let mut ui = opendeskphone_ui::Ui::from_hal(&mut hal, store.clone(), config);
        ui.set_fixed_time(Some((5_000_000, 1_794_607_087)));
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        common::answer_commands(&store, Domain::Provisioning, commands.clone(), |_| Ok(Reply::Done));
        let shell = opendeskphone_apps::uishell::UiShell::new(ui.handle(), model);
        let mut cfg = opendeskphone_apps::ManagerConfig::rooted(common::staged(), &dir.0.join("nvdata"));
        cfg.target = opendeskphone_apps::engine::Target::Pulley64;
        let services = services_with_store(&FakePbx::default(), fake, &FakeStore::default());
        let mut mgr =
            opendeskphone_apps::AppManager::new(cfg, store.clone(), Box::new(shell), services).expect("engine");
        mgr.scan();
        let mut phone = TouchPhone { ui, sim, mgr, commands, _dir: dir };
        phone.settle();
        phone
    }

    /// The UI and the manager take turns until both are done.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.ui.pump();
            self.mgr.pump();
        }
        self.ui.pump();
    }

    /// Whether the picture has a region that does `hit`.
    fn has(&self, hit: opendeskphone_ui::Hit) -> bool {
        self.ui.canvas().hit_rect(hit).is_some()
    }

    /// Touches the middle of the region that does `hit`.
    fn touch(&mut self, hit: opendeskphone_ui::Hit) {
        let r = self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?}"));
        self.sim.touch_tap((r.x + r.w / 2) as u16, (r.y + r.h / 2) as u16);
        self.settle();
    }

    /// Touches the keys of the on-screen keyboard that type `text`.
    fn type_keys(&mut self, text: &str) {
        for ch in text.chars() {
            self.touch(opendeskphone_ui::osk::Cap::Char(ch).hit());
        }
    }

    /// Writes the screen as a PNG, to look at.
    fn shot(&self, name: &str) {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("apps");
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (800, 1280);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let p = self.ui.canvas().pixel(x as i32, y as i32);
                rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        let file = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("D80-{name}.png"))).unwrap());
        let mut enc = png::Encoder::new(file, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
}

#[test]
fn hotdesking_is_logged_in_to_with_the_on_screen_keyboard() {
    use opendeskphone_ui::Hit;
    use opendeskphone_ui::osk::Cap;
    let mut p = TouchPhone::new();
    p.mgr.launch("hotdesking").unwrap();
    p.settle();
    assert_eq!(p.ui.screen_name(), "app:hotdesking");
    // The form opens with the extension focused: the number pad is up,
    // where the softkey buttons would be.
    assert!(p.has(Cap::Char('1').hit()) && p.has(Cap::Char('.').hit()) && !p.has(Cap::Char('q').hit()));
    assert!(!p.has(Hit::Key(Key::Soft(1))));
    p.type_keys("1011");
    p.touch(Cap::Backspace.hit());
    p.shot("hotdesking-extension");
    // Done: the app moves on to the PIN, whose pad has digits only.
    p.touch(Cap::Done.hit());
    assert!(p.has(Cap::Char('1').hit()) && !p.has(Cap::Char('.').hit()));
    p.type_keys("4321");
    p.shot("hotdesking-pin");
    // Done on the last field logs in.
    p.touch(Cap::Done.hit());
    assert!(
        p.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::SelectUser { id, pin } if id == "101" && pin == "4321"))
    );
    assert!(!p.has(Cap::Done.hit()));
}

#[test]
fn the_keyboard_is_put_away_for_hotdesking_s_softkeys() {
    use opendeskphone_ui::Hit;
    use opendeskphone_ui::osk::Cap;
    let mut p = TouchPhone::new();
    p.mgr.launch("hotdesking").unwrap();
    p.settle();
    p.type_keys("101");
    // The back button puts the keyboard away: the softkeys are buttons
    // again, and the app is still there, with what was typed.
    p.sim.touch_tap(48, 48);
    p.settle();
    assert_eq!(p.ui.screen_name(), "app:hotdesking");
    assert!(!p.has(Cap::Done.hit()) && p.has(Hit::Key(Key::Soft(1))));
    p.shot("hotdesking-keyboard-away");
    // The back button now leaves the app.
    p.sim.touch_tap(48, 48);
    p.settle();
    assert_ne!(p.ui.screen_name(), "app:hotdesking");
}

#[test]
fn the_calendar_on_the_touchscreen_in_a_picture() {
    // The evening of November 13, 2026, seven hours west.
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1_794_618_000));
    let mut p = TouchPhone::with(FakeConfig { utc_offset: -7 * 3600, wall_time: Some(clock), ..FakeConfig::dasp() });
    p.mgr.launch("calendar").unwrap();
    p.settle();
    assert_eq!(p.ui.screen_name(), "app:calendar");
    p.shot("calendar");
    // Under the title, the month's name between two buttons; under that
    // the weekdays (from 208, 66 high) and the five weeks of the month,
    // 171 high each, nearly to the bottom of the window.
    let c = p.ui.canvas();
    let ground = c.pixel(400, 1150);
    assert_ne!(c.pixel(16 + 1, 104 + 48), ground, "the button to the month before");
    assert_ne!(c.pixel(800 - 16 - 2, 104 + 48), ground, "the button to the month after");
    // The mark on today (Friday the 13th: the sixth column, the second
    // row of days) is a disc in the middle of the day's place: its middle
    // is marked, and the corners of the place are not.
    let col = 800 / 7;
    let (x0, y0) = (5 * col + col / 2, 274 + 171 + 85);
    let mark = c.pixel(x0, y0 - 30);
    assert_ne!(mark, ground, "today is marked");
    assert_eq!(c.pixel(x0 - 40, y0 - 40), ground, "a disc, not a square");
    let run = |dy: i32| (1..400).take_while(|i| c.pixel(x0 + 25, y0 + dy * i) == mark).count();
    let height = run(-1) + run(1) + 1;
    assert!((50..80).contains(&height), "{height}");
    // The month after: "Today" is the way back.
    p.sim.touch_tap(800 - 16 - 48, 104 + 48);
    p.settle();
    p.shot("calendar-next");
    assert!(p.has(opendeskphone_ui::Hit::Key(Key::Soft(1))), "the Today button");
}
