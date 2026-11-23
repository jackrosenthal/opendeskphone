//! End-to-end tests of the Bluetooth, paging, expansion, and alarms services and
//! the serial console switch: the real server on a loopback port, driven
//! over raw HTTP/1.1 with the JSON codec.

use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{self as st, Command, CommandError, Domain, Model, Reply, Store};
use opendeskphone_web::{WebOptions, WebServer};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Phone {
    addr: SocketAddr,
    store: Store,
    config: Arc<Mutex<ConfigStore>>,
    dir: PathBuf,
    /// Commands the stub handlers received.
    seen: Arc<Mutex<Vec<Command>>>,
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn phone(model: Model) -> Phone {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).unwrap();
    let dir = std::env::temp_dir().join(format!("opendeskphone-web-devices-{}", id.map(|b| format!("{b:02x}")).concat()));
    std::fs::create_dir_all(dir.join("nvdata")).unwrap();
    let nv = Nvdata::new(dir.join("nvdata"));
    nv.ensure_layout().unwrap();
    let cfg_model: opendeskphone_config::Model = model.name().parse().unwrap();
    let config = Arc::new(Mutex::new(ConfigStore::open(nv, cfg_model, dir.join("tmp")).unwrap()));
    let store = Store::new(model);
    let seen = Arc::new(Mutex::new(Vec::new()));
    for domain in [Domain::Bluetooth, Domain::Paging, Domain::Alarms] {
        let rx = store.handle(domain);
        let seen = seen.clone();
        std::thread::spawn(move || {
            while let Some(env) = rx.recv() {
                let reply = match &env.command {
                    Command::BluetoothConnect { .. } => Err(CommandError::Failed("the headset did not answer".into())),
                    Command::SetAlarm { alarm } if alarm.id == 0 => Ok(Reply::Id(5)),
                    Command::SnoozeAlarm => Err(CommandError::Invalid("no alarm is ringing".into())),
                    _ => Ok(Reply::Done),
                };
                seen.lock().unwrap().push(env.command.clone());
                env.reply(reply);
            }
        });
    }
    let options = WebOptions {
        http: Some("127.0.0.1:0".parse().unwrap()),
        https: None,
        upload_dir: dir.join("upload"),
        ..Default::default()
    };
    let bound = WebServer::new(store.clone(), config.clone(), options).bind().await.unwrap();
    let addr = bound.http_addr().unwrap();
    tokio::spawn(bound.serve());
    Phone { addr, store, config, dir, seen }
}

struct Login {
    cookie: String,
    csrf: String,
}

/// One request on a fresh connection.  With `first_frame`, returns once
/// the first stream message has arrived (for streams that do not end).
async fn post(addr: SocketAddr, path: &str, headers: &[(&str, &str)], body: &[u8], first_frame: bool) -> (u16, Vec<u8>, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let dechunk = |raw: &[u8]| -> Option<(String, Vec<u8>)> {
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut data = &raw[split + 4..];
        if !head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
            return Some((head, data.to_vec()));
        }
        let mut out = Vec::new();
        while let Some(eol) = data.windows(2).position(|w| w == b"\r\n") {
            let len = usize::from_str_radix(std::str::from_utf8(&data[..eol]).ok()?.trim(), 16).ok()?;
            if len == 0 || data.len() < eol + 2 + len + 2 {
                break;
            }
            out.extend_from_slice(&data[eol + 2..eol + 2 + len]);
            data = &data[eol + 2 + len + 2..];
        }
        Some((head, out))
    };
    loop {
        if first_frame
            && let Some((_, body)) = dechunk(&raw)
            && body.len() >= 5
            && body.len() >= 5 + u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize
        {
            break;
        }
        match tokio::time::timeout_at(deadline, s.read(&mut buf)).await.expect("response in time").unwrap() {
            0 => break,
            n => raw.extend_from_slice(&buf[..n]),
        }
    }
    let (head, body) = dechunk(&raw).expect("a response");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, body, head)
}

async fn login(addr: SocketAddr) -> Login {
    let (status, body, head) =
        post(addr, "/opendeskphone.v1.AuthService/Login", &[("Content-Type", "application/json")], br#"{"user":"admin","password":"789"}"#, false).await;
    assert_eq!(status, 200);
    let cookie = head
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: ").or_else(|| l.strip_prefix("Set-Cookie: ")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let v: Value = serde_json::from_slice(&body).unwrap();
    Login { cookie, csrf: v["session"]["csrfToken"].as_str().unwrap().to_owned() }
}

async fn call(p: &Phone, l: &Login, method: &str, body: Value) -> (u16, Value) {
    let (status, body, _) = post(
        p.addr,
        &format!("/opendeskphone.v1.{method}"),
        &[("Content-Type", "application/json"), ("Cookie", &l.cookie), ("X-Csrf-Token", &l.csrf)],
        body.to_string().as_bytes(),
        false,
    )
    .await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The first message of a watch stream.
async fn first(p: &Phone, l: &Login, method: &str) -> Value {
    let mut framed = vec![0u8, 0, 0, 0, 2];
    framed.extend(b"{}");
    let (status, body, _) = post(
        p.addr,
        &format!("/opendeskphone.v1.{method}"),
        &[("Content-Type", "application/connect+json"), ("Cookie", &l.cookie), ("X-Csrf-Token", &l.csrf)],
        &framed,
        true,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body[0], 0, "a data frame");
    let len = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
    serde_json::from_slice(&body[5..5 + len]).unwrap()
}

fn last_command(p: &Phone) -> Command {
    p.seen.lock().unwrap().last().cloned().expect("a command")
}

#[tokio::test]
async fn bluetooth() {
    let p = phone(Model::D65).await;
    let l = login(p.addr).await;
    let (_, v) = call(&p, &l, "StatusService/GetStatus", json!({})).await;
    assert_eq!(v["status"]["system"]["hasBluetooth"], true);

    p.store.update::<st::Bluetooth, _>(|b| {
        b.present = true;
        b.powered = true;
        b.address = "00:0F:D3:65:00:01".into();
        b.discovering = true;
        b.found = vec![st::BluetoothDevice { addr: "70:BF:92:11:22:33".into(), name: "Evolve2 65".into(), rssi: -48 }];
        b.paired = vec![st::BluetoothDevice { addr: "00:1B:DC:0F:A1:02".into(), name: "Voyager 5200".into(), rssi: 0 }];
        b.pairing =
            Some(st::BluetoothPairing { addr: "70:BF:92:11:22:33".into(), name: "Evolve2 65".into(), passkey: Some(42), pin_needed: false });
        b.connected = "00:1B:DC:0F:A1:02".into();
    });
    let expected = json!({
        "present": true,
        "powered": true,
        "address": "00:0F:D3:65:00:01",
        "discovering": true,
        "found": [{ "address": "70:BF:92:11:22:33", "name": "Evolve2 65", "rssi": -48 }],
        "paired": [{ "address": "00:1B:DC:0F:A1:02", "name": "Voyager 5200" }],
        "pairing": { "address": "70:BF:92:11:22:33", "name": "Evolve2 65", "passkey": 42 },
        "connected": "00:1B:DC:0F:A1:02",
    });
    let (status, v) = call(&p, &l, "BluetoothService/GetBluetooth", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(v["snapshot"], expected);
    assert_eq!(first(&p, &l, "BluetoothService/WatchBluetooth").await["snapshot"], expected);

    // Each RPC becomes its command.
    let cases: Vec<(&str, Value, Command)> = vec![
        ("SetBluetoothDiscovery", json!({ "discovering": true }), Command::BluetoothDiscovery { on: true }),
        // Addresses are normalized to upper case.
        ("PairBluetoothDevice", json!({ "address": "70:bf:92:11:22:33" }), Command::BluetoothPair { addr: "70:BF:92:11:22:33".into() }),
        (
            "AnswerBluetoothPairing",
            json!({ "address": "70:BF:92:11:22:33", "accept": true }),
            Command::BluetoothAnswerPairing { addr: "70:BF:92:11:22:33".into(), accept: true, pin: None },
        ),
        (
            "AnswerBluetoothPairing",
            json!({ "address": "70:BF:92:11:22:33", "accept": true, "pin": "0000" }),
            Command::BluetoothAnswerPairing { addr: "70:BF:92:11:22:33".into(), accept: true, pin: Some("0000".into()) },
        ),
        (
            "AnswerBluetoothPairing",
            json!({ "address": "70:BF:92:11:22:33" }),
            Command::BluetoothAnswerPairing { addr: "70:BF:92:11:22:33".into(), accept: false, pin: None },
        ),
        ("RemoveBluetoothDevice", json!({ "address": "00:1B:DC:0F:A1:02" }), Command::BluetoothUnpair { addr: "00:1B:DC:0F:A1:02".into() }),
        ("DisconnectBluetoothDevice", json!({}), Command::BluetoothDisconnect),
    ];
    for (method, body, command) in cases {
        let (status, v) = call(&p, &l, &format!("BluetoothService/{method}"), body).await;
        assert_eq!(status, 200, "{method}: {v}");
        assert_eq!(format!("{:?}", last_command(&p)), format!("{command:?}"));
    }

    // A failure from the subsystem reaches the caller with its text.
    let (status, v) = call(&p, &l, "BluetoothService/ConnectBluetoothDevice", json!({ "address": "00:1B:DC:0F:A1:02" })).await;
    assert_ne!(status, 200);
    assert_eq!(v["message"], "the headset did not answer");

    // Bad input is refused before any command is sent.
    let sent = p.seen.lock().unwrap().len();
    for (method, body) in [
        ("PairBluetoothDevice", json!({ "address": "nonsense" })),
        ("PairBluetoothDevice", json!({ "address": "70:BF:92:11:22" })),
        ("RemoveBluetoothDevice", json!({ "address": "70-BF-92-11-22-33" })),
        ("AnswerBluetoothPairing", json!({ "address": "70:BF:92:11:22:33", "accept": true, "pin": "12345678901234567" })),
    ] {
        let (status, v) = call(&p, &l, &format!("BluetoothService/{method}"), body).await;
        assert_eq!((status, v["code"].as_str()), (400, Some("invalid_argument")), "{method}");
    }
    assert_eq!(p.seen.lock().unwrap().len(), sent);

    // Not without a session.
    let (status, _, _) =
        post(p.addr, "/opendeskphone.v1.BluetoothService/SetBluetoothDiscovery", &[("Content-Type", "application/json")], b"{}", false).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn no_bluetooth_on_other_models() {
    let p = phone(Model::D60).await;
    let l = login(p.addr).await;
    let (_, v) = call(&p, &l, "StatusService/GetStatus", json!({})).await;
    assert_eq!(v["status"]["system"].get("hasBluetooth"), None);
    assert_eq!(v["status"]["system"]["hasExpansion"], true);
    let (status, v) = call(&p, &l, "BluetoothService/GetBluetooth", json!({})).await;
    assert_eq!((status, &v["snapshot"]), (200, &json!({})));
    let (status, v) = call(&p, &l, "BluetoothService/SetBluetoothDiscovery", json!({ "discovering": true })).await;
    assert_eq!((status, v["code"].as_str()), (501, Some("unimplemented")));
    assert!(p.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn paging_and_expansion() {
    let p = phone(Model::D60).await;
    let l = login(p.addr).await;
    p.store.update::<st::Paging, _>(|g| {
        g.listeners = vec![st::PageGroup { id: "1".into(), name: "All phones".into(), addr: "239.1.1.1".into(), port: 5004 }];
        g.broadcasts = vec![st::PageGroup { id: "3".into(), name: "Front office".into(), addr: "239.1.1.3".into(), port: 5006 }];
        g.activity = st::PageActivity::Receiving { group: "1".into(), name: "All phones".into() };
    });
    let expected = json!({
        "listeners": [{ "id": "1", "name": "All phones", "address": "239.1.1.1", "port": 5004 }],
        "broadcasts": [{ "id": "3", "name": "Front office", "address": "239.1.1.3", "port": 5006 }],
        "activity": "PAGE_ACTIVITY_RECEIVING",
        "activeGroup": "1",
        "activeName": "All phones",
    });
    let (_, v) = call(&p, &l, "PagingService/GetPaging", json!({})).await;
    assert_eq!(v["snapshot"], expected);
    assert_eq!(first(&p, &l, "PagingService/WatchPaging").await["snapshot"], expected);

    let (status, _) = call(&p, &l, "PagingService/StartPage", json!({ "id": "3" })).await;
    assert_eq!(status, 200);
    assert!(matches!(last_command(&p), Command::StartPage { id } if id == "3"));
    // Only a group this phone may page.
    let (status, v) = call(&p, &l, "PagingService/StartPage", json!({ "id": "1" })).await;
    assert_eq!((status, v["code"].as_str()), (404, Some("not_found")));
    let (status, _) = call(&p, &l, "PagingService/StopPage", json!({})).await;
    assert_eq!(status, 200);
    assert!(matches!(last_command(&p), Command::StopPage));

    p.store.update::<st::Expansion, _>(|e| {
        e.enabled = true;
        e.modules = vec![st::ExpansionModule { node: 1, addr: "192.168.218.3".into(), version: "EXP150 1_2_0".into(), page: 2 }];
        e.error = "second module lost".into();
    });
    let expected = json!({
        "enabled": true,
        "modules": [{ "node": 1, "address": "192.168.218.3", "version": "EXP150 1_2_0", "page": 2 }],
        "error": "second module lost",
    });
    let (_, v) = call(&p, &l, "ExpansionService/GetExpansion", json!({})).await;
    assert_eq!(v["snapshot"], expected);
    assert_eq!(first(&p, &l, "ExpansionService/WatchExpansion").await["snapshot"], expected);
}

#[tokio::test]
async fn serial_console_switch() {
    let p = phone(Model::D60).await;
    let l = login(p.addr).await;
    let settings = p.dir.join("nvdata/opendeskphone/settings.txtpb");
    // The EHS headset adapter has the port unless the console is asked for.
    let (_, v) = call(&p, &l, "SystemService/GetSerialConsole", json!({})).await;
    assert_eq!(v["serialConsole"], json!({ "source": "SETTING_SOURCE_DEFAULT" }));

    let (status, v) = call(&p, &l, "SystemService/SetSerialConsole", json!({ "serialConsole": true })).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["serialConsole"], json!({ "value": true, "source": "SETTING_SOURCE_LOCAL" }));
    assert!(std::fs::read_to_string(&settings).unwrap().contains("serial_console: true"));
    assert!(p.config.lock().unwrap().config().serial_console().value);
    let (_, v) = call(&p, &l, "SystemService/SetSerialConsole", json!({})).await;
    assert_eq!(v["serialConsole"], json!({ "source": "SETTING_SOURCE_LOCAL" }));

    // A value the provisioning server froze cannot be changed here.
    p.config
        .lock()
        .unwrap()
        .apply_fetched(r#"<config><setting id="opendeskphone_serial_console" value="1" conflict="freeze"/></config>"#)
        .unwrap();
    let (_, v) = call(&p, &l, "SystemService/GetSerialConsole", json!({})).await;
    assert_eq!(v["serialConsole"], json!({ "value": true, "source": "SETTING_SOURCE_PROVISIONED", "locked": true }));
    let (status, v) = call(&p, &l, "SystemService/SetSerialConsole", json!({})).await;
    assert_eq!((status, v["code"].as_str()), (400, Some("failed_precondition")));
}

#[tokio::test]
async fn idle_softkeys() {
    let p = phone(Model::D40).await;
    let l = login(p.addr).await;
    let settings = p.dir.join("nvdata/opendeskphone/settings.txtpb");
    // Nothing set: the default list.
    let (_, v) = call(&p, &l, "ConfigService/GetIdleSoftkeys", json!({})).await;
    let v = &v["idleSoftkeys"];
    let functions = |names: &[&str]| -> serde_json::Value {
        names.iter().map(|n| json!({ "function": format!("FUNCTION_{n}") })).collect()
    };
    assert_eq!(v["source"], "SETTING_SOURCE_DEFAULT");

    let list = json!([{ "function": "FUNCTION_MENU" }, { "app": { "name": "parking" } }, { "function": "FUNCTION_DND" }]);
    let (status, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({ "softkeys": list })).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["idleSoftkeys"]["softkeys"], list);
    // The functions, then the phone's apps but the contacts and voicemail
    // apps; a softkey in effect whose app the phone does not have (absent,
    // or switched off) is still listed.
    p.store.update::<st::Apps, _>(|a| {
        for (name, display_name) in [("queues", "Queues"), ("voicemail", "Voicemail"), ("contacts", "Contacts")] {
            a.installed.push(st::AppInfo { name: name.into(), display_name: display_name.into(), ..Default::default() });
        }
    });
    let (_, v) = call(&p, &l, "ConfigService/GetIdleSoftkeys", json!({})).await;
    let choices = v["idleSoftkeys"]["choices"].as_array().unwrap();
    assert_eq!(choices[8], json!({ "softkey": { "function": "FUNCTION_VOICEMAIL" }, "label": "Voicemail" }));
    assert_eq!(choices[9], json!({ "softkey": { "app": { "name": "queues" } }, "label": "Queues (app)" }));
    assert_eq!(choices[10], json!({ "softkey": { "app": { "name": "parking" } }, "label": "parking" }));
    assert_eq!(choices.len(), 11);
    // A softkey in effect for the voicemail or the contacts app keeps its
    // entry, labeled as the other apps are.
    let apps = json!([{ "app": { "name": "voicemail" } }, { "app": { "name": "contacts" } }]);
    let (_, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({ "softkeys": apps })).await;
    let choices = v["idleSoftkeys"]["choices"].as_array().unwrap();
    assert_eq!(choices[10], json!({ "softkey": { "app": { "name": "voicemail" } }, "label": "Voicemail (app)" }));
    assert_eq!(choices[11], json!({ "softkey": { "app": { "name": "contacts" } }, "label": "Contacts (app)" }));
    assert_eq!(choices.len(), 12);
    let (_, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({ "softkeys": list })).await;
    assert_eq!(v["idleSoftkeys"]["softkeys"], list);
    assert_eq!(v["idleSoftkeys"]["source"], "SETTING_SOURCE_LOCAL");
    assert!(std::fs::read_to_string(&settings).unwrap().contains(
        "idle_softkeys {\n  function: FUNCTION_MENU\n}\nidle_softkeys {\n  app {\n    name: \"parking\"\n  }\n}\nidle_softkeys {\n  function: FUNCTION_DND\n}\n"
    ));

    // Not softkeys: no function, no action at all, and an app name that
    // is not one.
    for bad in [json!({ "function": "FUNCTION_UNSPECIFIED" }), json!({}), json!({ "app": {} }), json!({ "app": { "name": "a,b" } })] {
        let list = json!([{ "function": "FUNCTION_MENU" }, bad]);
        let (status, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({ "softkeys": list })).await;
        assert_eq!((status, v["code"].as_str()), (400, Some("invalid_argument")), "{v}");
    }

    // An empty list drops the local one.
    let (_, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({})).await;
    assert_eq!(v["idleSoftkeys"]["source"], "SETTING_SOURCE_DEFAULT");
    assert!(!std::fs::read_to_string(&settings).unwrap().contains("idle_softkeys"));

    // A list the provisioning server froze cannot be changed here.
    p.config
        .lock()
        .unwrap()
        .apply_fetched(r#"<config><setting id="opendeskphone_idle_softkeys" value="call_log,menu" conflict="freeze"/></config>"#)
        .unwrap();
    let (_, v) = call(&p, &l, "ConfigService/GetIdleSoftkeys", json!({})).await;
    assert_eq!(v["idleSoftkeys"]["softkeys"], functions(&["CALL_LOG", "MENU"]));
    assert_eq!(v["idleSoftkeys"]["locked"], true);
    let (status, v) = call(&p, &l, "ConfigService/SetIdleSoftkeys", json!({ "softkeys": functions(&["MENU"]) })).await;
    assert_eq!((status, v["code"].as_str()), (400, Some("failed_precondition")));
}

#[tokio::test]
async fn alarms() {
    let p = phone(Model::D60).await;
    let l = login(p.addr).await;
    p.store.update::<st::Alarms, _>(|a| {
        a.alarms = vec![
            st::Alarm { id: 1, hour: 8, minute: 0, days: 0b0000010, tone: "Osmium".into(), enabled: true },
            st::Alarm { id: 2, hour: 0, minute: 5, days: 0, tone: "Beep".into(), enabled: false },
        ];
        a.tones = vec!["Beep".into(), "Osmium".into()];
        a.icon = true;
        a.next = Some((1, 1_794_844_800));
        a.snoozed = Some((2, 1_794_800_000));
        a.snooze_secs = 600;
    });
    let expected = json!({
        "alarms": [
            { "id": 1, "hour": 8, "days": 2, "tone": "Osmium", "enabled": true },
            { "id": 2, "minute": 5, "tone": "Beep" },
        ],
        "tones": ["Beep", "Osmium"],
        "next": { "id": 1, "unixSecs": "1794844800" },
        "snoozed": { "id": 2, "unixSecs": "1794800000" },
        "snoozeSecs": 600,
        "maxAlarms": 16,
    });
    let (status, v) = call(&p, &l, "AlarmsService/GetAlarms", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(v["snapshot"], expected);
    assert_eq!(first(&p, &l, "AlarmsService/WatchAlarms").await["snapshot"], expected);
    p.store.update::<st::Alarms, _>(|a| a.ringing = Some(st::AlarmRinging { id: 1, hour: 8, minute: 0 }));
    let (_, v) = call(&p, &l, "AlarmsService/GetAlarms", json!({})).await;
    assert_eq!(v["snapshot"]["ringing"], 1);

    // Each RPC becomes its command.
    let weekly = st::Alarm { id: 0, hour: 6, minute: 30, days: 0b0111110, tone: "Osmium".into(), enabled: true };
    let cases: Vec<(&str, Value, Command)> = vec![
        (
            "SetAlarm",
            json!({ "alarm": { "hour": 6, "minute": 30, "days": 62, "tone": "Osmium", "enabled": true } }),
            Command::SetAlarm { alarm: weekly.clone() },
        ),
        (
            "SetAlarm",
            json!({ "alarm": { "id": 2, "hour": 23, "minute": 59 } }),
            Command::SetAlarm { alarm: st::Alarm { id: 2, hour: 23, minute: 59, days: 0, tone: String::new(), enabled: false } },
        ),
        ("RemoveAlarm", json!({ "id": 1 }), Command::RemoveAlarm { id: 1 }),
        ("DismissAlarm", json!({}), Command::DismissAlarm),
    ];
    for (method, body, command) in cases {
        let (status, v) = call(&p, &l, &format!("AlarmsService/{method}"), body).await;
        assert_eq!(status, 200, "{method}: {v}");
        assert_eq!(last_command(&p), command);
        // A new alarm's ID comes back, and a changed alarm's is its own.
        match command {
            Command::SetAlarm { alarm } if alarm.id == 0 => assert_eq!(v["id"], 5),
            Command::SetAlarm { alarm } => assert_eq!(v["id"], alarm.id),
            _ => {}
        }
    }

    // A refusal from the alarm clock reaches the caller with its text.
    let (status, v) = call(&p, &l, "AlarmsService/SnoozeAlarm", json!({})).await;
    assert_ne!(status, 200);
    assert_eq!(v["message"], "no alarm is ringing");

    // Bad input is refused before any command is sent.
    let sent = p.seen.lock().unwrap().len();
    for (method, body, code) in [
        ("SetAlarm", json!({}), "invalid_argument"),
        ("SetAlarm", json!({ "alarm": { "hour": 24 } }), "invalid_argument"),
        ("SetAlarm", json!({ "alarm": { "hour": 1, "minute": 60 } }), "invalid_argument"),
        ("SetAlarm", json!({ "alarm": { "hour": 1, "days": 128 } }), "invalid_argument"),
        ("SetAlarm", json!({ "alarm": { "id": 9, "hour": 1 } }), "not_found"),
        ("RemoveAlarm", json!({ "id": 9 }), "not_found"),
    ] {
        let (status, v) = call(&p, &l, &format!("AlarmsService/{method}"), body.clone()).await;
        assert_ne!(status, 200, "{method} {body}");
        assert_eq!(v["code"].as_str(), Some(code), "{method} {body}");
    }
    // No more than the phone keeps.
    p.store.update::<st::Alarms, _>(|a| {
        a.alarms = (1..=16).map(|id| st::Alarm { id, ..weekly.clone() }).collect();
    });
    let (_, v) = call(&p, &l, "AlarmsService/SetAlarm", json!({ "alarm": { "hour": 6 } })).await;
    assert_eq!(v["code"], "resource_exhausted");
    assert_eq!(p.seen.lock().unwrap().len(), sent);

    // Not without a session.
    let (status, _, _) = post(p.addr, "/opendeskphone.v1.AlarmsService/DismissAlarm", &[("Content-Type", "application/json")], b"{}", false).await;
    assert_eq!(status, 401);
}
