//! End-to-end tests: the real server on a loopback port, driven over raw
//! HTTP/1.1 so that the wire format itself is what is checked.

use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{Command, Domain, ForwardKind, Key, Model, Reply, Store};
use opendeskphone_web::{pb, FileFirmwareSink, FirmwareInfo, WebOptions, WebServer};
use buffa::Message;
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
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn phone() -> Phone {
    phone_of(Model::D60).await
}

async fn phone_of(model: Model) -> Phone {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).unwrap();
    let dir = std::env::temp_dir().join(format!("opendeskphone-web-test-{}", id.map(|b| format!("{b:02x}")).concat()));
    std::fs::create_dir_all(dir.join("nvdata")).unwrap();
    let nv = Nvdata::new(dir.join("nvdata"));
    nv.ensure_layout().unwrap();
    let config = Arc::new(Mutex::new(ConfigStore::open(nv, model.name().parse().unwrap(), dir.join("tmp")).unwrap()));
    let store = Store::new(model);
    let options = WebOptions {
        http: Some("127.0.0.1:0".parse().unwrap()),
        https: None,
        upload_dir: dir.join("upload"),
        max_firmware_bytes: 1 << 20,
        ..Default::default()
    };
    let sink = FileFirmwareSink::new(dir.join("upload")).with_inspector(|path| {
        let data = std::fs::read(path)?;
        if data.starts_with(b"STOCK") {
            Ok(FirmwareInfo { version: "2_9_28".into(), model: "D60".into(), is_stock: true })
        } else if data.starts_with(b"OPEN") {
            Ok(FirmwareInfo { version: "3_0_1".into(), model: "D60".into(), is_stock: false })
        } else if data.starts_with(b"D70") {
            Ok(FirmwareInfo { version: "3_0_1".into(), model: "D70".into(), is_stock: false })
        } else if data.starts_with(b"D99") {
            Ok(FirmwareInfo { version: "3_0_1".into(), model: "D99".into(), is_stock: false })
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no label"))
        }
    });
    let bound = WebServer::new(store.clone(), config.clone(), options)
        .with_firmware_sink(Box::new(sink))
        .bind()
        .await
        .unwrap();
    let addr = bound.http_addr().unwrap();
    tokio::spawn(bound.serve());
    Phone { addr, store, config, dir }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&self.body)))
    }
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = data.windows(2).position(|w| w == b"\r\n") {
        let Ok(len) = usize::from_str_radix(std::str::from_utf8(&data[..eol]).unwrap_or("").trim(), 16) else { break };
        let start = eol + 2;
        if len == 0 || data.len() < start + len {
            break;
        }
        out.extend_from_slice(&data[start..start + len]);
        data = &data[(start + len + 2).min(data.len())..];
    }
    out
}

/// Sends one request on a fresh connection.  With `want` set, returns as
/// soon as that many body bytes have arrived (for streams that never end).
async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    want: Option<usize>,
) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16384];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n");
        if let (Some(split), Some(want)) = (split, want)
            && dechunk(&raw[split + 4..]).len() >= want {
                break;
            }
        match tokio::time::timeout_at(deadline, s.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => raw.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => panic!("read: {e}"),
            Err(_) => panic!("timed out; got {:?}", String::from_utf8_lossy(&raw)),
        }
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':').map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))).collect();
    let chunked = headers.iter().any(|(n, v)| n.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let body = if chunked { dechunk(&raw[split + 4..]) } else { raw[split + 4..].to_vec() };
    Resp { status, headers, body }
}

struct Session {
    cookie: String,
    csrf: String,
}

async fn login(addr: SocketAddr, password: &str) -> Result<Session, Resp> {
    let body = format!(r#"{{"user":"admin","password":"{password}"}}"#);
    let r = request(addr, "POST", "/opendeskphone.v1.AuthService/Login", &[("Content-Type", "application/json")], body.as_bytes(), None).await;
    if r.status != 200 {
        return Err(r);
    }
    let cookie = r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    assert!(r.header("set-cookie").unwrap().contains("HttpOnly"));
    assert!(r.header("set-cookie").unwrap().contains("SameSite=Strict"));
    let csrf = r.json()["session"]["csrfToken"].as_str().unwrap().to_owned();
    Ok(Session { cookie, csrf })
}

async fn rpc_json(addr: SocketAddr, s: &Session, method: &str, body: &str) -> Resp {
    request(
        addr,
        "POST",
        &format!("/opendeskphone.v1.{method}"),
        &[("Content-Type", "application/json"), ("Cookie", &s.cookie), ("X-Csrf-Token", &s.csrf)],
        body.as_bytes(),
        None,
    )
    .await
}

#[tokio::test]
async fn static_files_and_authentication() {
    let p = phone().await;
    let r = request(p.addr, "GET", "/", &[], b"", None).await;
    assert_eq!(r.status, 200);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    assert!(r.header("content-security-policy").is_some());
    assert_eq!(r.header("x-frame-options"), Some("DENY"));
    let etag = r.header("etag").unwrap().to_owned();
    let again = request(p.addr, "GET", "/", &[("If-None-Match", &etag)], b"", None).await;
    assert_eq!(again.status, 304);
    // Client-side routes get the shell; missing files do not.
    assert_eq!(request(p.addr, "GET", "/accounts", &[], b"", None).await.status, 200);
    assert_eq!(request(p.addr, "GET", "/missing.js", &[], b"", None).await.status, 404);
    assert_eq!(request(p.addr, "PUT", "/", &[], b"", None).await.status, 405);

    // No session: every service refuses, in the Connect error shape.
    for method in ["StatusService/GetStatus", "ConfigService/ListSettings", "SystemService/Reboot", "AuthService/ChangePassword"] {
        let r = request(p.addr, "POST", &format!("/opendeskphone.v1.{method}"), &[("Content-Type", "application/json")], b"{}", None).await;
        assert_eq!(r.status, 401, "{method}");
        assert_eq!(r.json()["code"], "unauthenticated");
    }
    for path in ["/upload/firmware", "/upload/app"] {
        assert_eq!(request(p.addr, "POST", path, &[], b"x", None).await.status, 401);
    }
    // GetSession works without one and says so.
    let r = request(p.addr, "POST", "/opendeskphone.v1.AuthService/GetSession", &[("Content-Type", "application/json")], b"{}", None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["session"]["model"], "MODEL_D60");
    assert!(r.json()["session"].get("authenticated").is_none());

    assert_eq!(login(p.addr, "wrong").await.err().unwrap().status, 401);
    let s = login(p.addr, "789").await.ok().unwrap();

    // The cookie alone is not enough, and a foreign Origin is refused.
    let r = request(p.addr, "POST", "/opendeskphone.v1.StatusService/GetStatus", &[("Content-Type", "application/json"), ("Cookie", &s.cookie)], b"{}", None).await;
    assert_eq!(r.status, 403);
    let r = request(
        p.addr,
        "POST",
        "/opendeskphone.v1.StatusService/GetStatus",
        &[("Content-Type", "application/json"), ("Cookie", &s.cookie), ("X-Csrf-Token", &s.csrf), ("Origin", "http://evil.test")],
        b"{}",
        None,
    )
    .await;
    assert_eq!(r.status, 403);
    let origin = format!("http://{}", p.addr);
    let r = request(
        p.addr,
        "POST",
        "/opendeskphone.v1.StatusService/GetStatus",
        &[("Content-Type", "application/json"), ("Cookie", &s.cookie), ("X-Csrf-Token", &s.csrf), ("Origin", &origin)],
        b"{}",
        None,
    )
    .await;
    assert_eq!(r.status, 200);

    let r = rpc_json(p.addr, &s, "StatusService/GetStatus", "{}").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.json()["status"]["system"]["model"], "MODEL_D60");
    assert_eq!(r.json()["status"]["system"]["skin"], "SKIN_COLOR");
    assert_eq!(r.json()["status"]["health"][0]["subsystem"], "SUBSYSTEM_CORE");

    // The screen's layout: the model, its skin, and its keys.
    let v = rpc_json(p.addr, &s, "ScreenService/GetScreenLayout", "{}").await.json();
    assert_eq!((&v["model"], &v["skin"]), (&serde_json::json!("MODEL_D60"), &serde_json::json!("SKIN_COLOR")));
    let keys = v["keys"].as_array().unwrap();
    assert_eq!(keys[0], serde_json::json!({ "kind": "KIND_LINE", "index": 1 }));
    assert!(keys.contains(&serde_json::json!({ "kind": "KIND_DIGIT" })));
    assert!(keys.contains(&serde_json::json!({ "kind": "KIND_VOLUME_UP" })));
    assert_eq!(v.get("touch"), None, "not a touchscreen");

    // Unknown methods, wrong content types, and bad messages.
    assert_eq!(rpc_json(p.addr, &s, "StatusService/Nope", "{}").await.json()["code"], "unimplemented");
    let r = request(p.addr, "POST", "/opendeskphone.v1.StatusService/GetStatus", &[("Content-Type", "text/plain"), ("Cookie", &s.cookie), ("X-Csrf-Token", &s.csrf)], b"{}", None).await;
    assert_eq!(r.status, 415);
    let r = rpc_json(p.addr, &s, "CallsService/Dial", "{not json").await;
    assert_eq!((r.status, r.json()["code"].as_str().unwrap().to_owned()), (400, "invalid_argument".to_owned()));

    // Logout ends the session.
    assert_eq!(rpc_json(p.addr, &s, "AuthService/Logout", "{}").await.status, 200);
    assert_eq!(rpc_json(p.addr, &s, "StatusService/GetStatus", "{}").await.status, 401);
}

#[tokio::test]
async fn protobuf_codec_and_streaming() {
    let p = phone().await;
    let s = login(p.addr, "789").await.ok().unwrap();
    let auth = [("Cookie", s.cookie.as_str()), ("X-Csrf-Token", s.csrf.as_str())];

    // Unary, binary codec.
    let mut headers = vec![("Content-Type", "application/proto")];
    headers.extend_from_slice(&auth);
    let r = request(p.addr, "POST", "/opendeskphone.v1.StatusService/GetStatus", &headers, &pb::GetStatusRequest {}.encode_to_vec(), None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/proto"));
    let status = pb::GetStatusResponse::decode_from_slice(&r.body[..]).unwrap().status.unwrap();
    assert_eq!(status.system.unwrap().model.as_known(), Some(pb::Model::MODEL_D60));

    // Server streaming, binary codec: enveloped request, enveloped messages.
    p.store.log_line(opendeskphone_state::Level::Warn, "test", "first line".into());
    let msg = pb::TailLogsRequest { backlog: 10, min_level: Default::default() }.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend((msg.len() as u32).to_be_bytes());
    framed.extend(&msg);
    let mut headers = vec![("Content-Type", "application/connect+proto")];
    headers.extend_from_slice(&auth);
    let r = request(p.addr, "POST", "/opendeskphone.v1.LogsService/TailLogs", &headers, &framed, Some(6)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/connect+proto"));
    assert_eq!(r.body[0], 0, "a data frame");
    let len = u32::from_be_bytes(r.body[1..5].try_into().unwrap()) as usize;
    let first = pb::TailLogsResponse::decode_from_slice(&r.body[5..5 + len]).unwrap();
    assert!(first.entries.iter().any(|e| e.message == "first line" && e.level == pb::LogLevel::Warn));

    // Streaming, JSON codec.
    let json = br#"{"backlog": 5}"#;
    let mut framed = vec![0u8];
    framed.extend((json.len() as u32).to_be_bytes());
    framed.extend(json);
    let mut headers = vec![("Content-Type", "application/connect+json")];
    headers.extend_from_slice(&auth);
    let r = request(p.addr, "POST", "/opendeskphone.v1.LogsService/TailLogs", &headers, &framed, Some(6)).await;
    let len = u32::from_be_bytes(r.body[1..5].try_into().unwrap()) as usize;
    let v: serde_json::Value = serde_json::from_slice(&r.body[5..5 + len]).unwrap();
    assert_eq!(v["entries"][0]["level"], "LOG_LEVEL_WARN");

    // A stream that fails: HTTP 200, then only the end-of-stream frame
    // (flag 2) carrying the error.
    let json = br#"{"screen": "SCREEN_ID_SIDE"}"#;
    let mut framed = vec![0u8];
    framed.extend((json.len() as u32).to_be_bytes());
    framed.extend(json);
    let r = request(p.addr, "POST", "/opendeskphone.v1.ScreenService/WatchScreen", &headers, &framed, None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body[0], 2);
    let v: serde_json::Value = serde_json::from_slice(&r.body[5..]).unwrap();
    assert_eq!(v["error"]["code"], "not_found");

    // A stream follows the store: a watch sends again when state changes.
    let json = b"{}";
    let mut framed = vec![0u8];
    framed.extend((json.len() as u32).to_be_bytes());
    framed.extend(json);
    let store = p.store.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        store.update::<opendeskphone_state::Features, _>(|f| f.dnd = true);
    });
    let r = request(p.addr, "POST", "/opendeskphone.v1.CallsService/WatchCalls", &headers, &framed, Some(220)).await;
    let text = String::from_utf8_lossy(&r.body);
    assert!(text.contains(r#""dnd":true"#), "{text}");

    // Unary and streaming framings are not interchangeable.
    let r = rpc_json(p.addr, &s, "LogsService/TailLogs", "{}").await;
    assert_eq!(r.json()["code"], "unimplemented");
}

#[tokio::test]
async fn configuration_accounts_and_contacts() {
    let p = phone().await;
    let s = login(p.addr, "789").await.ok().unwrap();

    let r = rpc_json(p.addr, &s, "ConfigService/SetSettings", r#"{"changes":[{"id":"ringer_volume","value":"9"}]}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(p.config.lock().unwrap().get("ringer_volume").as_deref(), Some("9"));
    let v = rpc_json(p.addr, &s, "ConfigService/ListSettings", r#"{"prefix":"ringer_"}"#).await.json();
    assert_eq!(v["settings"][0]["id"], "ringer_volume");
    assert_eq!(v["settings"][0]["value"], "9");
    // A value set on the phone is not the server's; a setting only the
    // server gave is.
    p.config
        .lock()
        .unwrap()
        .apply_fetched(r#"<config><setting id="ringer_volume" value="3"/><setting id="handset_volume" value="4"/></config>"#)
        .unwrap();
    let v = rpc_json(p.addr, &s, "ConfigService/ListSettings", r#"{"prefix":"ringer_"}"#).await.json();
    assert_eq!(v["settings"][0]["value"], "9");
    assert_ne!(v["settings"][0]["provisioned"], true);
    let v = rpc_json(p.addr, &s, "ConfigService/ListSettings", r#"{"prefix":"handset_"}"#).await.json();
    assert_eq!(v["settings"][0]["value"], "4");
    assert_eq!(v["settings"][0]["provisioned"], true);
    // Secrets never leave the phone, and the password has its own RPC.
    let v = rpc_json(p.addr, &s, "ConfigService/ListSettings", r#"{"prefix":"login_"}"#).await.json();
    assert_eq!(v["settings"][0]["secret"], true);
    assert!(v["settings"][0].get("value").is_none());
    let r = rpc_json(p.addr, &s, "ConfigService/SetSettings", r#"{"changes":[{"id":"login_password","value":"x"}]}"#).await;
    assert_eq!(r.status, 400);
    let r = rpc_json(p.addr, &s, "ConfigService/SetSettings", r#"{"changes":[{"id":"bad id","value":"x"}]}"#).await;
    assert_eq!(r.status, 400);

    // Accounts: written in the stock shape; the password stays put.
    let body = r#"{"accounts":[{"index":0,"enabled":true,"register":true,"username":"100","password":"pw1","server":"pbx.test","transport":"SIP_TRANSPORT_TLS","srtp":true,"lineLabel":"Desk"}]}"#;
    let r = rpc_json(p.addr, &s, "AccountsService/SetAccounts", body).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    {
        let cfg = p.config.lock().unwrap().config();
        let a = &cfg.accounts()[0];
        assert_eq!((a.username.as_str(), a.password.as_str(), a.line_label.as_str()), ("100", "pw1", "Desk"));
        let h = a.primary.as_ref().unwrap();
        assert_eq!((h.server.as_str(), h.port, h.media_encryption.as_deref()), ("pbx.test", 5061, Some("sdes")));
    }
    let v = rpc_json(p.addr, &s, "AccountsService/ListAccounts", "{}").await.json();
    assert_eq!(v["accounts"][0]["hasPassword"], true);
    assert!(v["accounts"][0].get("password").is_none());
    assert_eq!(v["local"], true);
    assert_eq!(v["maxAccounts"], 2);
    // An edit without a password keeps the stored one.
    let body = r#"{"accounts":[{"index":0,"enabled":true,"register":true,"username":"100","server":"pbx2.test"}]}"#;
    assert_eq!(rpc_json(p.addr, &s, "AccountsService/SetAccounts", body).await.status, 200);
    {
        let cfg = p.config.lock().unwrap().config();
        assert_eq!(cfg.accounts()[0].password, "pw1");
        assert_eq!(cfg.accounts()[0].primary.as_ref().unwrap().server, "pbx2.test");
    }
    let body = r#"{"accounts":[{"index":5,"username":"1","server":"x"}]}"#;
    assert_eq!(rpc_json(p.addr, &s, "AccountsService/SetAccounts", body).await.status, 400);

    // Network settings validate before anything is stored.
    let r = rpc_json(p.addr, &s, "NetworkService/SetNetworkSettings", r#"{"settings":{"dhcp":false,"staticIp":"10.0.0.5","subnetMask":"255.0.255.0","vlanDiscoveryMode":"VLAN_DISCOVERY_MODE_NONE"}}"#).await;
    assert_eq!(r.status, 400);
    let r = rpc_json(p.addr, &s, "NetworkService/SetNetworkSettings", r#"{"settings":{"dhcp":true}}"#).await;
    assert_eq!(r.status, 400);
    let r = rpc_json(p.addr, &s, "NetworkService/SetNetworkSettings", r#"{"settings":{"dhcp":false,"staticIp":"10.0.0.5","subnetMask":"255.255.255.0","gateway":"10.0.0.1","vlanDiscoveryMode":"VLAN_DISCOVERY_MODE_MANUAL","vlanId":30,"sipDscp":26,"rtpDscp":46,"sipQos":3,"rtpQos":6,"pcQos":1}}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let net = p.config.lock().unwrap().config().network();
    assert!(!net.dhcp);
    assert_eq!(net.static_ip, Some("10.0.0.5".parse().unwrap()));
    assert_eq!(net.vlan_id, Some(30));
    assert_eq!(net.vlan_discovery_mode, "MANUAL");
    let v = rpc_json(p.addr, &s, "NetworkService/GetNetwork", "{}").await.json();
    assert_eq!(v["settings"]["vlanDiscoveryMode"], "VLAN_DISCOVERY_MODE_MANUAL");

    // Contacts: write, read back, export in the stock XML, delete.
    let body = r#"{"name":"local.xml","groups":[{"id":"g","name":"Office","contacts":[{"firstName":"Morgan","lastName":"Li","numbers":[{"dial":"104","label":"Ext","primary":true},{"dial":"5551230104"}]}]}]}"#;
    let r = rpc_json(p.addr, &s, "ContactsService/PutContactList", body).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let v = rpc_json(p.addr, &s, "ContactsService/GetContactList", r#"{"name":"local.xml"}"#).await.json();
    assert_eq!(v["groups"][0]["contacts"][0]["numbers"][0]["dial"], "104");
    assert_eq!(v["groups"][0]["contacts"][0]["numbers"][0]["primary"], true);
    let v = rpc_json(p.addr, &s, "ContactsService/ExportContactList", r#"{"name":"local.xml"}"#).await.json();
    let xml = v["xml"].as_str().unwrap();
    assert!(xml.contains("<phonebooks>") && xml.contains("first_name=\"Morgan\""), "{xml}");
    let v = rpc_json(p.addr, &s, "ContactsService/ListContactLists", "{}").await.json();
    assert_eq!(v["lists"][0]["contacts"], 1);
    for bad in ["../x", "a/b", ".hidden", ""] {
        let r = rpc_json(p.addr, &s, "ContactsService/GetContactList", &format!(r#"{{"name":"{bad}"}}"#)).await;
        assert_eq!(r.status, 400, "{bad}");
    }
    assert_eq!(rpc_json(p.addr, &s, "ContactsService/DeleteContactList", r#"{"name":"local.xml"}"#).await.status, 200);
    assert_eq!(rpc_json(p.addr, &s, "ContactsService/GetContactList", r#"{"name":"local.xml"}"#).await.status, 404);
}

#[tokio::test]
async fn commands_uploads_backup_and_password() {
    let p = phone().await;
    let s = login(p.addr, "789").await.ok().unwrap();

    // No subsystem registered: unavailable, not a hang.
    let r = rpc_json(p.addr, &s, "CallsService/Dial", r#"{"target":"100"}"#).await;
    assert_eq!((r.status, r.json()["code"].as_str().unwrap().to_owned()), (503, "unavailable".to_owned()));

    // With a handler, the command and its reply go through.
    let rx = p.store.handle(Domain::Calls);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            seen2.lock().unwrap().push(env.command.clone());
            env.reply(Ok(Reply::Call(opendeskphone_state::CallId(7))));
        }
    });
    let r = rpc_json(p.addr, &s, "CallsService/Dial", r#"{"target":"100","account":1}"#).await;
    assert_eq!(r.json()["call"], 7);
    assert_eq!(
        seen.lock().unwrap()[0],
        Command::Dial { target: "100".into(), account: Some(opendeskphone_state::AccountId(1)), path: None }
    );
    // A forwarding rule is named by its kind.
    let r = rpc_json(p.addr, &s, "CallsService/SetForward", r#"{"kind":"FORWARD_KIND_BUSY","target":"200"}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(seen.lock().unwrap()[1], Command::SetForward { kind: ForwardKind::Busy, target: "200".into(), no_answer_secs: None });
    assert_eq!(rpc_json(p.addr, &s, "CallsService/SetForward", r#"{"target":"200"}"#).await.status, 400);

    // A key is a kind, and an index where the kind has several keys.
    let rx = p.store.handle(Domain::Ui);
    let pressed = Arc::new(Mutex::new(Vec::new()));
    let pressed2 = pressed.clone();
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            pressed2.lock().unwrap().push(env.command.clone());
            env.done();
        }
    });
    let r = rpc_json(p.addr, &s, "ScreenService/PressKey", r#"{"key":{"kind":"KIND_LINE","index":2},"action":"KEY_ACTION_DOWN"}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let r = rpc_json(p.addr, &s, "ScreenService/PressKey", r#"{"key":{"kind":"KIND_EXPANSION","module":1,"index":17},"action":"KEY_ACTION_UP"}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        *pressed.lock().unwrap(),
        [
            Command::InjectKey { key: Key::Line(2), down: true },
            Command::InjectKey { key: Key::Expansion { module: 1, key: 17 }, down: false },
        ]
    );
    for bad in [r#"{}"#, r#"{"key":{}}"#, r#"{"key":{"kind":"KIND_SOFT","index":5}}"#, r#"{"key":{"kind":"KIND_DIGIT","index":10}}"#] {
        assert_eq!(rpc_json(p.addr, &s, "ScreenService/PressKey", bad).await.status, 400, "{bad}");
    }
    // A D60 has no touchscreen: nothing is sent to the UI.
    let r = rpc_json(p.addr, &s, "ScreenService/Touch", r#"{"x":10,"y":10}"#).await;
    assert_eq!((r.status, r.json()["code"].as_str()), (400, Some("failed_precondition")));
    assert_eq!(pressed.lock().unwrap().len(), 2);

    // Firmware upload: streamed to the sink, described, then installed.
    let up = |body: &'static [u8]| {
        let (cookie, csrf) = (s.cookie.clone(), s.csrf.clone());
        async move { request(p.addr, "POST", "/upload/firmware", &[("Cookie", &cookie), ("X-Csrf-Token", &csrf)], body, None).await }
    };
    assert_eq!(up(b"garbage").await.status, 400);
    let r = up(b"STOCK image body").await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let v = r.json();
    assert_eq!((v["isStock"].as_bool(), v["version"].as_str(), v["size"].as_str()), (Some(true), Some("2_9_28"), Some("16")));
    assert_eq!((&v["model"], &v["modelMatches"]), (&serde_json::json!("MODEL_D60"), &serde_json::json!(true)));
    let id = v["uploadId"].as_str().unwrap().to_owned();
    let v = rpc_json(p.addr, &s, "FirmwareService/GetFirmware", "{}").await.json();
    assert_eq!(v["status"]["uploaded"]["uploadId"], id.as_str());
    // A stock image needs the explicit flag.
    let r = rpc_json(p.addr, &s, "FirmwareService/InstallFirmware", &format!(r#"{{"uploadId":"{id}"}}"#)).await;
    assert_eq!(r.status, 403);
    let rx = p.store.handle(Domain::Update);
    let got = Arc::new(Mutex::new(None));
    let got2 = got.clone();
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            *got2.lock().unwrap() = Some(env.command.clone());
            env.done();
        }
    });
    let r = rpc_json(p.addr, &s, "FirmwareService/InstallFirmware", &format!(r#"{{"uploadId":"{id}","allowStock":true}}"#)).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    match got.lock().unwrap().clone() {
        Some(Command::InstallFirmware { path, allow_stock }) => {
            assert!(allow_stock);
            assert_eq!(std::fs::read(path).unwrap(), b"STOCK image body");
        }
        other => panic!("{other:?}"),
    }
    // An image for another model, and one for a model this firmware does
    // not know.
    let v = up(b"D70 image body").await.json();
    assert_eq!((&v["model"], v.get("modelMatches")), (&serde_json::json!("MODEL_D70"), None));
    let v = up(b"D99 image body").await.json();
    assert_eq!((v.get("model"), v.get("modelMatches")), (None, None));
    // Too large is refused from the declared length.
    let big = vec![b'O'; (1 << 20) + 1];
    let r = request(p.addr, "POST", "/upload/firmware", &[("Cookie", &s.cookie), ("X-Csrf-Token", &s.csrf)], &big, None).await;
    assert_eq!(r.status, 429);

    // Backup and restore.
    rpc_json(p.addr, &s, "ConfigService/SetSettings", r#"{"changes":[{"id":"ringer_volume","value":"2"}]}"#).await;
    let v = rpc_json(p.addr, &s, "SystemService/CreateBackup", "{}").await.json();
    assert!(v["filename"].as_str().unwrap().ends_with("-backup.tar.gz"));
    let archive = v["archive"].as_str().unwrap().to_owned();
    rpc_json(p.addr, &s, "ConfigService/SetSettings", r#"{"changes":[{"id":"ringer_volume","value":"7"}]}"#).await;
    let r = rpc_json(p.addr, &s, "SystemService/RestoreBackup", &format!(r#"{{"archive":"{archive}"}}"#)).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(p.config.lock().unwrap().get("ringer_volume").as_deref(), Some("2"));
    assert_eq!(rpc_json(p.addr, &s, "SystemService/RestoreBackup", r#"{"archive":"anVuaw=="}"#).await.status, 400);

    // Factory reset needs the model typed.
    assert_eq!(rpc_json(p.addr, &s, "SystemService/FactoryReset", r#"{"confirm":"yes"}"#).await.status, 400);

    // Password change: stock digest file, other sessions end.
    let other = login(p.addr, "789").await.ok().unwrap();
    let r = rpc_json(p.addr, &s, "AuthService/ChangePassword", r#"{"currentPassword":"bad","newPassword":"hunter22"}"#).await;
    assert_eq!(r.status, 403);
    let r = rpc_json(p.addr, &s, "AuthService/ChangePassword", r#"{"currentPassword":"789","newPassword":"hunter22"}"#).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let digest = std::fs::read_to_string(p.dir.join("nvdata/etc/.passwd")).unwrap();
    assert!(digest.starts_with("admin:Digium Phone Admin Login:"), "{digest}");
    assert_eq!(rpc_json(p.addr, &other, "StatusService/GetStatus", "{}").await.status, 401);
    assert_eq!(rpc_json(p.addr, &s, "StatusService/GetStatus", "{}").await.status, 200);
    assert!(login(p.addr, "789").await.is_err());
    assert!(login(p.addr, "hunter22").await.is_ok());
}

#[tokio::test]
async fn login_throttling() {
    let p = phone().await;
    for _ in 0..3 {
        assert_eq!(login(p.addr, "nope").await.err().unwrap().status, 401);
    }
    // Locked out now, even with the right password.
    let r = login(p.addr, "789").await.err().unwrap();
    assert_eq!(r.status, 429);
    assert_eq!(r.json()["code"], "resource_exhausted");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(login(p.addr, "789").await.is_ok());
}

/// A static address from the provisioning server is what the network form
/// shows, and saving the form leaves the phone's own address settings alone.
#[tokio::test]
async fn provisioned_static_address_in_the_network_form() {
    let p = phone().await;
    let s = login(p.addr, "789").await.ok().unwrap();
    p.config
        .lock()
        .unwrap()
        .apply_fetched(
            r#"<config><setting id="opendeskphone_static_address" value="198.18.14.120/23"/><setting id="opendeskphone_static_gateway" value="198.18.14.1"/></config>"#,
        )
        .unwrap();
    let v = rpc_json(p.addr, &s, "NetworkService/GetNetwork", "{}").await.json();
    let form = &v["settings"];
    assert_eq!(form["addressingProvisioned"], true);
    assert_ne!(form["dhcp"], true);
    assert_eq!(form["staticIp"], "198.18.14.120");
    assert_eq!(form["subnetMask"], "255.255.254.0");
    assert_eq!(form["gateway"], "198.18.14.1");
    let body = serde_json::json!({ "settings": form }).to_string();
    let r = rpc_json(p.addr, &s, "NetworkService/SetNetworkSettings", &body).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let config = p.config.lock().unwrap().config();
    assert!(config.network().dhcp, "the phone's own setting still says DHCP");
    assert_eq!(config.network().static_ip, None);
}

/// The theme: the built-in ones are listed, a choice is kept with
/// Open Desk Phone's own settings, and what the provisioning server fixed
/// cannot be changed.
#[tokio::test]
async fn the_theme_setting() {
    let p = phone().await;
    let s = login(p.addr, "789").await.ok().unwrap();
    let (addr, s) = (p.addr, &s);
    let get = || async move { rpc_json(addr, s, "ConfigService/GetTheme", "{}").await.json()["theme"].clone() };
    let set = |id: &'static str| async move {
        rpc_json(addr, s, "ConfigService/SetTheme", &serde_json::json!({ "id": id }).to_string()).await
    };
    let w = get().await;
    assert_eq!((&w["id"], &w["source"]), (&serde_json::json!("tides"), &serde_json::json!("SETTING_SOURCE_DEFAULT")));
    let choices = w["choices"].as_array().unwrap();
    assert_eq!(choices.len(), opendeskphone_state::THEMES.len());
    assert_eq!(choices[0], serde_json::json!({ "id": "harbor", "name": "Harbor", "family": "Facets" }));

    // A choice is saved in the settings file and is in effect.
    let r = set("dusk").await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["theme"]["id"], "dusk");
    assert_eq!(r.json()["theme"]["source"], "SETTING_SOURCE_LOCAL");
    let config = p.config.lock().unwrap().config();
    assert_eq!(config.opendeskphone_settings().theme.as_deref(), Some("dusk"));
    // Not a theme: refused, and nothing changes.
    let r = set("no-such").await;
    assert_eq!(r.json()["code"], "invalid_argument");
    assert_eq!(get().await["id"], "dusk");
    // A name in the file that this firmware does not have is the default
    // theme, as on the phone's screen.
    p.config.lock().unwrap().update_opendeskphone_settings(|d| d.theme = Some("from-the-future".into())).unwrap();
    assert_eq!(get().await["id"], "tides");

    // The provisioning server's picture, frozen as a DPMA server sends
    // it: it is the wallpaper, and the theme cannot be changed here.
    p.config
        .lock()
        .unwrap()
        .apply_fetched(r#"<config><setting id="wallpaper_file" value="user" url="file://wall.png" conflict="freeze"/></config>"#)
        .unwrap();
    let w = get().await;
    assert_eq!((&w["picture"], &w["locked"], &w["source"]), (&serde_json::json!(true), &serde_json::json!(true), &serde_json::json!("SETTING_SOURCE_PROVISIONED")));
    assert_eq!(set("tides").await.json()["code"], "failed_precondition");
    // A provisioned name, not frozen: shown as the server's until one is
    // chosen here; an empty choice puts the server's back.
    p.config.lock().unwrap().update_opendeskphone_settings(|d| d.theme = None).unwrap();
    p.config.lock().unwrap().apply_fetched(r#"<config><setting id="opendeskphone_theme" value="slate"/></config>"#).unwrap();
    let w = get().await;
    assert_eq!((&w["id"], &w["source"]), (&serde_json::json!("slate"), &serde_json::json!("SETTING_SOURCE_PROVISIONED")));
    assert!(w.get("picture").is_none() && w.get("locked").is_none());
    assert_eq!(set("tides").await.json()["theme"]["source"], "SETTING_SOURCE_LOCAL");
    let r = set("").await;
    assert_eq!((&r.json()["theme"]["id"], &r.json()["theme"]["source"]), (&serde_json::json!("slate"), &serde_json::json!("SETTING_SOURCE_PROVISIONED")));
}

/// The D80's screen is a touchscreen: the layout says so and lists no
/// keys, and a touch is a place and what the finger did there.
#[tokio::test]
async fn touches_of_the_touchscreen() {
    use opendeskphone_state::TouchPhase::*;
    let p = phone_of(Model::D80).await;
    let s = login(p.addr, "789").await.ok().unwrap();

    let v = rpc_json(p.addr, &s, "ScreenService/GetScreenLayout", "{}").await.json();
    assert_eq!((&v["model"], &v["skin"], &v["touch"]), (&serde_json::json!("MODEL_D80"), &serde_json::json!("SKIN_TOUCH"), &serde_json::json!(true)));
    assert_eq!(v["screens"], serde_json::json!([{ "id": "SCREEN_ID_MAIN", "width": 800, "height": 1280, "color": true }]));
    assert_eq!(v.get("keys"), None, "no keys");

    let rx = p.store.handle(Domain::Ui);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            seen2.lock().unwrap().push(env.command.clone());
            env.done();
        }
    });
    // A drag, each event as it comes; a tap, which is down and up at one
    // place (also with no action given); and a touch called off.
    for body in [
        r#"{"x":400,"y":1000,"action":"TOUCH_ACTION_DOWN"}"#,
        r#"{"x":400,"y":700,"action":"TOUCH_ACTION_MOVE"}"#,
        r#"{"x":399,"y":300,"action":"TOUCH_ACTION_UP"}"#,
        r#"{"x":0,"y":1279,"action":"TOUCH_ACTION_TAP"}"#,
        r#"{"x":799,"y":0}"#,
        r#"{"x":5,"y":6,"action":"TOUCH_ACTION_DOWN"}"#,
        r#"{"x":5,"y":6,"action":"TOUCH_ACTION_CANCEL"}"#,
    ] {
        let r = rpc_json(p.addr, &s, "ScreenService/Touch", body).await;
        assert_eq!(r.status, 200, "{body}: {:?}", String::from_utf8_lossy(&r.body));
    }
    let t = |phase, x, y| Command::InjectTouch { phase, x, y };
    assert_eq!(
        *seen.lock().unwrap(),
        [
            t(Down, 400, 1000),
            t(Move, 400, 700),
            t(Up, 399, 300),
            t(Down, 0, 1279),
            t(Up, 0, 1279),
            t(Down, 799, 0),
            t(Up, 799, 0),
            t(Down, 5, 6),
            t(Cancel, 5, 6),
        ]
    );
    // Off the screen.
    for bad in [r#"{"x":800,"y":0}"#, r#"{"x":0,"y":1280}"#, r#"{"x":70000,"y":1}"#] {
        let r = rpc_json(p.addr, &s, "ScreenService/Touch", bad).await;
        assert_eq!((r.status, r.json()["code"].as_str()), (400, Some("invalid_argument")), "{bad}");
    }
    assert_eq!(seen.lock().unwrap().len(), 9);
}
