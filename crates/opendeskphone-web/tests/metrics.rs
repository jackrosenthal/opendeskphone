//! `/metrics`, end to end: the real server on a loopback port.

use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{self as st, Model, Store};
use opendeskphone_web::{WebOptions, WebServer};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Phone {
    addr: SocketAddr,
    store: Store,
    dir: PathBuf,
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn phone(prepare: impl FnOnce(&Store)) -> Phone {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).unwrap();
    let dir = std::env::temp_dir().join(format!("opendeskphone-metrics-test-{}", id.map(|b| format!("{b:02x}")).concat()));
    std::fs::create_dir_all(dir.join("nvdata")).unwrap();
    let nv = Nvdata::new(dir.join("nvdata"));
    nv.ensure_layout().unwrap();
    let config = Arc::new(Mutex::new(ConfigStore::open(nv, opendeskphone_config::Model::D60, dir.join("tmp")).unwrap()));
    let store = Store::new(Model::D60);
    prepare(&store);
    let options = WebOptions {
        http: Some("127.0.0.1:0".parse().unwrap()),
        https: None,
        upload_dir: dir.join("upload"),
        ..Default::default()
    };
    let bound = WebServer::new(store.clone(), config, options).bind().await.unwrap();
    let addr = bound.http_addr().unwrap();
    tokio::spawn(bound.serve());
    Phone { addr, store, dir }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

async fn request(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: 0\r\n");
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw)).await.expect("response").unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers =
        lines.filter_map(|l| l.split_once(':').map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))).collect();
    Resp { status, headers, body: String::from_utf8(raw[split + 4..].to_vec()).expect("UTF-8 body") }
}

// ------------------------------------------------- a strict format check

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

/// Parses `a="b",c="d"`, honoring the three escapes.
fn parse_labels(mut s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while !s.is_empty() {
        let (name, rest) = s.split_once("=\"").unwrap_or_else(|| panic!("label without a value: {s:?}"));
        assert!(is_name(name) && !name.contains(':'), "label name {name:?}");
        let mut value = String::new();
        let mut chars = rest.char_indices();
        let end = loop {
            let (i, c) = chars.next().unwrap_or_else(|| panic!("unterminated label value: {rest:?}"));
            match c {
                '"' => break i,
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('n') => value.push('\n'),
                    Some(c @ ('\\' | '"')) => value.push(c),
                    other => panic!("bad escape {other:?}"),
                },
                '\n' => panic!("raw newline in a label value"),
                c => value.push(c),
            }
        };
        out.push((name.to_owned(), value));
        s = &rest[end + 1..];
        if let Some(more) = s.strip_prefix(',') {
            assert!(!more.is_empty(), "trailing comma");
            s = more;
        } else {
            assert!(s.is_empty(), "junk after a label value: {s:?}");
        }
    }
    out
}

type Sample = (String, Vec<(String, String)>, f64);

/// Checks the body against the OpenMetrics text rules that matter to a
/// scraper and returns the samples.
fn parse(body: &str) -> Vec<Sample> {
    assert!(body.ends_with("# EOF\n"), "must end with # EOF");
    assert!(body.len() <= 20 << 10, "body is {} bytes", body.len());
    let mut types: HashMap<String, String> = HashMap::new();
    let mut help: HashSet<String> = HashSet::new();
    let mut finished: HashSet<String> = HashSet::new();
    let mut current: Option<String> = None;
    let mut series: HashSet<String> = HashSet::new();
    let mut samples = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    for (n, line) in lines.iter().enumerate() {
        assert!(!line.is_empty(), "blank line {n}");
        if *line == "# EOF" {
            assert_eq!(n, lines.len() - 1, "# EOF must be last");
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            let mut it = rest.splitn(3, ' ');
            let (kind, family, text) = (it.next().unwrap(), it.next().expect("family"), it.next().expect("text"));
            assert!(is_name(family), "family name {family:?}");
            if current.as_deref() != Some(family) {
                if let Some(done) = current.replace(family.to_owned()) {
                    finished.insert(done);
                }
                assert!(!finished.contains(family), "family {family} is not contiguous");
            }
            match kind {
                "HELP" => assert!(help.insert(family.to_owned()), "second HELP for {family}"),
                "TYPE" => {
                    assert!(matches!(text, "gauge" | "counter"), "type {text:?}");
                    assert!(types.insert(family.to_owned(), text.to_owned()).is_none(), "second TYPE for {family}");
                }
                other => panic!("unexpected comment {other:?}"),
            }
            continue;
        }
        let (name_labels, value) = line.rsplit_once(' ').unwrap_or_else(|| panic!("no value: {line:?}"));
        let (name, labels) = match name_labels.split_once('{') {
            Some((name, rest)) => {
                let labels = parse_labels(rest.strip_suffix('}').expect("closing brace"));
                assert!(!labels.is_empty(), "empty braces in {line:?}");
                (name, labels)
            }
            None => (name_labels, Vec::new()),
        };
        assert!(is_name(name), "metric name {name:?}");
        let family = current.as_deref().unwrap_or_else(|| panic!("sample before any family: {line:?}"));
        match types.get(family).map(String::as_str) {
            Some("counter") => assert_eq!(name, format!("{family}_total"), "counter samples end in _total"),
            Some("gauge") => assert_eq!(name, family),
            other => panic!("sample {name} in family {family} of type {other:?}"),
        }
        assert!(help.contains(family), "no HELP for {family}");
        let value: f64 = value.parse().unwrap_or_else(|_| panic!("value {value:?}"));
        assert!(value.is_finite(), "{line:?}");
        if types[family] == "counter" {
            assert!(value >= 0.0, "{line:?}");
        }
        let mut names: Vec<&str> = labels.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), labels.len(), "repeated label in {line:?}");
        assert!(series.insert(name_labels.to_owned()), "duplicate series {name_labels}");
        assert!(name.starts_with("opendeskphone_") || name.starts_with("process_"), "prefix of {name}");
        samples.push((name.to_owned(), labels, value));
    }
    samples
}

fn value(samples: &[Sample], name: &str, labels: &[(&str, &str)]) -> f64 {
    let found: Vec<&Sample> = samples
        .iter()
        .filter(|(n, l, _)| {
            n == name && labels.iter().all(|(k, v)| l.iter().any(|(lk, lv)| lk == k && lv == v)) && l.len() == labels.len()
        })
        .collect();
    assert_eq!(found.len(), 1, "{name}{labels:?}: {} matches", found.len());
    found[0].2
}

// ---------------------------------------------------------------- state

const SECRETS: [&str; 12] = [
    "front-desk-phone",
    "Avery Quill",
    "4155550123",
    "pbx.example.net",
    "sipsecretuser",
    "Jordan Marlowe",
    "2125550188",
    "jordan@far.example.org",
    "00:0f:d3:aa:bb:cc",
    "switch-closet-7",
    "prov.example.net",
    "assigned-user-77",
];

/// A phone with something in every part of the state, including text that
/// must never be published.
fn fabricate(store: &Store) {
    store.update::<st::System, _>(|s| {
        s.hostname = "front-desk-phone".into();
        s.mac = Some([0x00, 0x0f, 0xd3, 0xaa, 0xbb, 0xcc]);
        s.phase = st::Phase::Running;
        s.time_synced = true;
        s.last_reboot_reason = "Avery Quill pressed reboot".into();
    });
    store.update::<st::Network, _>(|n| {
        n.phase = st::NetPhase::Up;
        n.lan = st::PortLink { up: true, speed: 100, full_duplex: true };
        n.ipv4.source = st::AddrSource::Dhcp;
        n.ipv4.address = Some("198.18.15.87".parse().unwrap());
        n.ipv4.gateway = Some("198.18.15.1".parse().unwrap());
        n.ipv4.lease_secs = 3600;
        n.ipv4.lease_obtained_mono_ms = st::clock::mono_ms();
        n.domain = "pbx.example.net".into();
        n.lldp = Some(st::LldpNeighbor { system_name: "switch-closet-7".into(), ..Default::default() });
    });
    store.update::<st::Accounts, _>(|a| {
        let mut one = st::Account::new(st::AccountId(0));
        one.user = "sipsecretuser".into();
        one.display_name = "Avery Quill".into();
        one.server = "pbx.example.net".into();
        one.registration = st::Registration::Registered {
            host: st::ActiveHost::Primary,
            expires_mono_ms: st::clock::mono_ms() + 300_000,
        };
        let mut two = st::Account::new(st::AccountId(1));
        two.user = "4155550123".into();
        two.registration = st::Registration::Failed { code: 403, reason: "Forbidden sipsecretuser".into(), retry_mono_ms: 0 };
        a.accounts = vec![one, two];
    });
    store.update::<st::Calls, _>(|c| {
        let far = st::Party { name: "Jordan Marlowe".into(), number: "2125550188".into(), uri: "sip:jordan@far.example.org".into() };
        let mut active = st::Call::new(st::CallId(7), st::AccountId(0), st::Direction::Incoming, far.clone());
        active.state = st::CallState::Active;
        active.stats.jitter_ms = 12;
        let mut held = st::Call::new(st::CallId(8), st::AccountId(0), st::Direction::Outgoing, far);
        held.state = st::CallState::Held { local: true, remote: false };
        c.calls = vec![active, held];
    });
    store.update::<st::Provisioning, _>(|p| {
        p.phase = st::ProvisionPhase::Provisioned;
        p.attached = true;
        p.assigned_user = "assigned-user-77".into();
        p.server = Some(st::DiscoveredServer {
            source: st::DiscoverySource::Manual,
            name: "prov.example.net".into(),
            address: "prov.example.net:5060".into(),
        });
        p.last_success_unix = Some(1_800_000_000);
    });
    store.update::<st::Update, _>(|u| u.running_bank = Some(st::Bank::B));
    store.set_health(st::Subsystem::Sip, st::HealthStatus::Ok);
    store.set_health(st::Subsystem::Media, st::HealthStatus::Failed("DSP: Jordan Marlowe".into()));
    store.heartbeat(st::Subsystem::Sip);
    store.log_line(st::Level::Info, "sip", "call from Jordan Marlowe 2125550188".into());
}

fn ended(store: &Store, id: u32, direction: st::Direction, cause: st::EndCause, rx: u32) {
    let far = st::Party { name: "Jordan Marlowe".into(), number: "2125550188".into(), uri: String::new() };
    let mut call = st::Call::new(st::CallId(id), st::AccountId(0), direction, far);
    call.state = st::CallState::Ended(cause);
    call.stats = st::MediaStats { rx_packets: rx, tx_packets: rx + 1, rx_lost: 2, jitter_ms: 3 };
    store.emit(st::Event::CallEnded { call });
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn open_to_scrapers_and_nothing_else_is() {
    let p = phone(fabricate).await;

    // No credentials, no cookie, no CSRF token.
    let r = request(p.addr, "GET", "/metrics", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/plain; version=0.0.4; charset=utf-8"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(r.header("set-cookie").is_none() && r.header("www-authenticate").is_none());
    assert_eq!(r.header("content-length").unwrap().parse::<usize>().unwrap(), r.body.len());
    parse(&r.body);

    // What Prometheus sends.
    let accept = "application/openmetrics-text;version=1.0.0;q=0.5,text/plain;version=0.0.4;q=0.3,*/*;q=0.1";
    let r = request(p.addr, "GET", "/metrics", &[("Accept", accept)]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/openmetrics-text; version=1.0.0; charset=utf-8"));
    parse(&r.body);

    let r = request(p.addr, "HEAD", "/metrics", &[]).await;
    assert_eq!(r.status, 200);

    // Only the exact path, and only for reading.
    assert_eq!(request(p.addr, "POST", "/metrics", &[]).await.status, 404);
    assert_eq!(request(p.addr, "DELETE", "/metrics", &[]).await.status, 405);
    let r = request(p.addr, "GET", "/metrics/", &[]).await;
    assert!(!r.body.contains("opendeskphone_build_info"));

    // Everything else that is not a static file still needs a session.
    for path in [
        "/opendeskphone.v1.StatusService/GetStatus",
        "/opendeskphone.v1.ConfigService/GetSettings",
        "/opendeskphone.v1.ShellService/OpenShell",
        "/opendeskphone.v1.SshService/ListSshKeys",
        "/opendeskphone.v1.SystemService/Reboot",
        "/opendeskphone.v1.LogsService/GetLogs",
        "/upload/firmware",
        "/upload/app",
    ] {
        let r = request(p.addr, "POST", path, &[("Content-Type", "application/json")]).await;
        assert_eq!(r.status, 401, "{path}: {}", r.body);
        assert!(r.body.contains("unauthenticated"), "{path}: {}", r.body);
    }
}

#[tokio::test]
async fn values_follow_the_state() {
    let p = phone(fabricate).await;
    let r = request(p.addr, "GET", "/metrics", &[]).await;
    let m = parse(&r.body);

    assert_eq!(value(&m, "opendeskphone_build_info", &[("version", "3.1.0"), ("model", "D60")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_system_phase", &[("phase", "running")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_system_phase", &[("phase", "starting")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_time_synced", &[]), 1.0);
    assert_eq!(value(&m, "opendeskphone_subsystem_health", &[("subsystem", "sip"), ("status", "ok")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_subsystem_health", &[("subsystem", "media"), ("status", "failed")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_subsystem_health", &[("subsystem", "media"), ("status", "ok")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_subsystem_health", &[("subsystem", "dpma"), ("status", "stopped")]), 1.0);
    assert!(value(&m, "opendeskphone_subsystem_heartbeat_age_seconds", &[("subsystem", "sip")]) < 10.0);
    assert_eq!(value(&m, "opendeskphone_network_phase", &[("phase", "up")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_network_link_up", &[("port", "lan")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_network_link_up", &[("port", "pc")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_network_link_speed_bytes", &[("port", "lan")]), 12_500_000.0);
    assert_eq!(value(&m, "opendeskphone_network_address_present", &[("family", "ipv4")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_network_address_present", &[("family", "ipv6")]), 0.0);
    let lease = value(&m, "opendeskphone_network_dhcp_lease_remaining_seconds", &[]);
    assert!((3500.0..=3600.0).contains(&lease), "{lease}");
    assert_eq!(value(&m, "opendeskphone_accounts", &[]), 2.0);
    assert_eq!(value(&m, "opendeskphone_account_registration", &[("account", "0"), ("state", "registered")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_account_registration", &[("account", "1"), ("state", "failed")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_account_registration", &[("account", "1"), ("state", "registered")]), 0.0);
    let expires = value(&m, "opendeskphone_account_registration_expires_seconds", &[("account", "0")]);
    assert!((200.0..=300.0).contains(&expires), "{expires}");
    assert_eq!(value(&m, "opendeskphone_calls_active", &[("state", "active")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_calls_active", &[("state", "held")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_calls_active", &[("state", "ringing")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_rtp_jitter_seconds", &[]), 0.012);
    assert_eq!(value(&m, "opendeskphone_calls_total", &[("direction", "incoming"), ("cause", "remote_hangup")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_provisioning_phase", &[("phase", "provisioned")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_provisioning_attached", &[]), 1.0);
    assert_eq!(value(&m, "opendeskphone_provisioning_last_success_timestamp_seconds", &[]), 1_800_000_000.0);
    assert_eq!(value(&m, "opendeskphone_firmware_running_bank", &[("bank", "b")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_firmware_running_bank", &[("bank", "a")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_firmware_trial", &[("state", "none")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_web_sessions", &[]), 0.0);
    assert!(value(&m, "opendeskphone_log_lines_total", &[]) >= 1.0);
    assert!(value(&m, "process_resident_memory_bytes", &[]) > 0.0);
    assert!(value(&m, "process_open_fds", &[]) >= 3.0);
    assert!(value(&m, "opendeskphone_memory_total_bytes", &[]) > 0.0);
    assert!(value(&m, "opendeskphone_nvdata_size_bytes", &[]) > 0.0);

    // Calls are counted as they end, whether or not anyone is scraping.
    ended(&p.store, 20, st::Direction::Incoming, st::EndCause::RemoteHangup, 1000);
    ended(&p.store, 21, st::Direction::Incoming, st::EndCause::RemoteHangup, 500);
    ended(&p.store, 22, st::Direction::Outgoing, st::EndCause::Failed { code: 486, reason: "Busy Here".into() }, 0);
    ended(&p.store, 23, st::Direction::Incoming, st::EndCause::Canceled { answered_elsewhere: true }, 0);
    p.store.update::<st::Calls, _>(|c| c.calls.clear());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let r = request(p.addr, "GET", "/metrics", &[]).await;
    let m = parse(&r.body);
    assert_eq!(value(&m, "opendeskphone_calls_total", &[("direction", "incoming"), ("cause", "remote_hangup")]), 2.0);
    assert_eq!(value(&m, "opendeskphone_calls_total", &[("direction", "outgoing"), ("cause", "failed")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_calls_total", &[("direction", "incoming"), ("cause", "answered_elsewhere")]), 1.0);
    assert_eq!(value(&m, "opendeskphone_calls_total", &[("direction", "outgoing"), ("cause", "remote_hangup")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_rtp_received_packets_total", &[]), 1500.0);
    assert_eq!(value(&m, "opendeskphone_rtp_sent_packets_total", &[]), 1504.0);
    assert_eq!(value(&m, "opendeskphone_rtp_lost_packets_total", &[]), 8.0);
    assert_eq!(value(&m, "opendeskphone_calls_active", &[("state", "active")]), 0.0);
    assert_eq!(value(&m, "opendeskphone_events_lost_total", &[]), 0.0);
}

#[tokio::test]
async fn nothing_that_identifies_anyone() {
    let p = phone(fabricate).await;
    ended(&p.store, 30, st::Direction::Incoming, st::EndCause::Error("Jordan Marlowe hung up".into()), 10);
    let r = request(p.addr, "GET", "/metrics", &[]).await;
    let lower = r.body.to_lowercase();
    for secret in SECRETS {
        assert!(!lower.contains(&secret.to_lowercase()), "{secret:?} is in the metrics");
    }
    for forbidden in ["198.18.15.", "forbidden", "example", "admin", "sip:"] {
        assert!(!lower.contains(forbidden), "{forbidden:?} is in the metrics");
    }
    // Every label comes from a fixed vocabulary.
    let allowed: HashSet<&str> = [
        "version", "model", "phase", "subsystem", "status", "port", "family", "account", "state", "direction",
        "cause", "bank", "window", "partition",
    ]
    .into();
    for (name, labels, _) in parse(&r.body) {
        for (label, value) in labels {
            assert!(allowed.contains(label.as_str()), "{name} has label {label}");
            assert!(
                value.len() <= 24 && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
                "{name}{{{label}={value:?}}}"
            );
        }
    }
}

#[tokio::test]
async fn repeated_requests_reuse_one_rendering() {
    let p = phone(fabricate).await;
    let first = request(p.addr, "GET", "/metrics", &[]).await.body;
    // A change inside the interval is not rendered again...
    p.store.update::<st::System, _>(|s| s.time_synced = false);
    for _ in 0..20 {
        assert_eq!(request(p.addr, "GET", "/metrics", &[]).await.body, first);
    }
    // ...and is there once the interval has passed.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let later = request(p.addr, "GET", "/metrics", &[]).await.body;
    assert_eq!(value(&parse(&later), "opendeskphone_time_synced", &[]), 0.0);
}
