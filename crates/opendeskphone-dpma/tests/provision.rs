//! The provisioning state machine against an in-process DPMA server made
//! of `dpma-proto`'s server-side pieces.

mod common;

use common::{wait, FakeServer, Mem, MemSink, TempFiles};
use opendeskphone_dpma::xml::Configurator;
use opendeskphone_dpma::{DpmaConfig, DpmaError, DpmaHandle};
use opendeskphone_state::{Command, DiscoveredServer, DiscoverySource, Event, Model, ProvisionPhase, Provisioning, Store, Topic};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

const CONFIG: &str = "<config>\n<setting id=\"login_password\" value=\"789\" />\n<firmwares>\n<firmware model=\"D60\" version=\"2_9_28\" url=\"http://pbx/f.eff\" network_id=\"n\" />\n</firmwares>\n<accounts><account id=\"100\" username=\"100\"/></accounts>\n</config>";

struct Rig {
    store: Store,
    server: Arc<Mutex<FakeServer>>,
    transport: Arc<Mem>,
    files: TempFiles,
    sink: Arc<MemSink>,
    dpma: DpmaHandle,
    to_phone: mpsc::UnboundedSender<opendeskphone_dpma::Message>,
}

fn rig_with(server: Arc<Mutex<FakeServer>>, files: TempFiles, sink: Arc<MemSink>) -> Rig {
    rig_timed(server, files, sink, Duration::from_millis(500))
}

/// [`rig_with`], waiting `timeout` for an answer.
fn rig_timed(server: Arc<Mutex<FakeServer>>, files: TempFiles, sink: Arc<MemSink>, timeout: Duration) -> Rig {
    let store = Store::new(Model::D60);
    let (to_phone, incoming) = mpsc::unbounded_channel();
    let transport = Arc::new(Mem { server: server.clone(), to_phone: to_phone.clone(), down: Mutex::new(false) });
    let mut cfg = DpmaConfig::new("00:0F:D3:AA:BB:CC", "Digium D60 3_0_0");
    cfg.timeout = timeout;
    cfg.retry_secs = 1;
    cfg.ping_secs = 0;
    let dpma = opendeskphone_dpma::start(store.clone(), cfg, transport.clone(), incoming, files.files(), sink.clone());
    Rig { store, server, transport, files, sink, dpma, to_phone }
}

fn rig() -> Rig {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    rig_with(Arc::new(Mutex::new(s)), TempFiles::default(), Arc::new(MemSink::default()))
}

fn phase(store: &Store) -> ProvisionPhase {
    store.get::<Provisioning>().phase.clone()
}

async fn until(store: &Store, what: &str, want: ProvisionPhase) {
    wait(what, || (phase(store) == want).then_some(())).await;
}

#[tokio::test(flavor = "current_thread")]
async fn first_boot_user_token_config_and_pushes() {
    let r = rig();
    let events = r.store.subscribe_events(Topic::Provisioning);
    let pushes = |seen: &mut Vec<(String, String)>| {
        while let Some(w) = events.try_take() {
            for e in w.events {
                if let Event::Push { name, body } = e {
                    seen.push((name, body));
                }
            }
        }
    };
    let mut seen = Vec::new();

    // No server known: wait for the user.
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "awaiting a server", ProvisionPhase::AwaitingServer).await;

    // DHCP option 66 in its sip: form names a server; the user picks it.
    r.dpma.network(true, Some("sip:proxy@pbx.test:5060".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test:5060".into()) }).await.unwrap();
    until(&r.store, "awaiting a user", ProvisionPhase::AwaitingUser).await;
    let p = r.store.get::<Provisioning>();
    assert_eq!(p.server.as_ref().unwrap().source, DiscoverySource::DhcpOption66);
    assert_eq!(p.users.len(), 1);
    assert_eq!((p.users[0].id.as_str(), p.users[0].name.as_str(), p.users[0].needs_pin), ("100", "Desk", true));
    assert_eq!(r.server.lock().unwrap().user_agent, "Digium D60 3_0_0");
    // The identity was minted and stored; the server verified it against
    // the provisioning CA.
    assert!(read_file(&r.files.identity_cert).starts_with(b"-----BEGIN CERTIFICATE-----"));
    assert!(read_file(&r.files.identity_key).starts_with(b"-----BEGIN RSA PRIVATE KEY-----"));

    // A wrong PIN is reported with the stock error code.
    r.store.send(Command::SelectUser { id: "100".into(), pin: "0000".into() }).await.unwrap();
    wait("PIN refused", || matches!(phase(&r.store), ProvisionPhase::Failed { code: Some(-199), .. }).then_some(())).await;

    // The right one provisions.
    r.store.send(Command::SelectUser { id: "100".into(), pin: "1234".into() }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    let p = r.store.get::<Provisioning>();
    assert!(p.attached);
    assert_eq!(p.assigned_user, "100");
    assert_eq!(p.offered_firmware.as_deref(), Some("2_9_28"));
    let applied = r.sink.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    let text = String::from_utf8(applied[0].clone()).unwrap();
    assert!(text.contains("login_password") && text.contains("<accounts>"));
    assert!(text.contains("<firmware model=\"D60\""), "provisioned firmware is passed on by default");
    // No digest from the server: the cache holds the MD5 of the download.
    assert_eq!(p.config_md5.len(), 32);
    assert_eq!(r.sink.md5.lock().unwrap().as_deref(), Some(p.config_md5.as_str()));
    // configurator.xml carries stock's settings.
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(c.get("config_server_type"), Some("boot66"));
    assert_eq!(c.get("config_server_url"), Some("sip:proxy@pbx.test:5060"));
    assert_eq!(c.get("config_server_user"), Some("100"));
    assert_eq!(c.get("config_user_password"), Some("1234"));
    assert!(c.get("config_server_token").unwrap().starts_with("100/000FD3AABBCC/"));

    // App requests ride the session.
    let doc = br#"{"request":{"method":"switchvox.users.voicemail.getList","parameters":{"account_id":"100"}}}"#;
    let reply = r.dpma.app_request("dasp://pbx.test/json", Some("100:1234"), doc).await.unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, br#"{"response":{"result":{"messages":[]}}}"#);
    assert_eq!(reply.headers[0].0, "Content-Type");
    assert_eq!(r.dpma.file("ringtone1").await.unwrap(), b"RIFFdata");
    assert_eq!(r.dpma.app_request("file://ringtone1", None, b"").await.unwrap().body, b"RIFFdata");
    assert!(matches!(r.dpma.file("nothing").await, Err(DpmaError::Server { code: -98, .. })));

    // So does app data, which the server keeps for the phone the token
    // names.
    use opendeskphone_dpma::appdata::{Op, Outcome, Scope};
    let set = |value: &[u8], expect| Op::Set { app: "game".into(), scope: Scope::Shared, key: "best".into(), value: value.to_vec(), expect };
    let stored = r.dpma.app_data(&set(b"53", Some(0))).await.unwrap();
    let user = stored.user.unwrap();
    assert_eq!((user.id.as_str(), user.name.as_str()), ("100", "Desk"));
    let Outcome::Stored(entry) = stored.outcome else { panic!("{:?}", stored.outcome) };
    assert_eq!((entry.value.as_slice(), entry.author_name.as_str()), (&b"53"[..], "Desk"));
    let again = r.dpma.app_data(&set(b"54", Some(0))).await.unwrap();
    assert_eq!(again.outcome, Outcome::Conflict(Some(entry.clone())));
    let got = r.dpma.app_data(&Op::Get { app: "game".into(), scope: Scope::Shared, key: "best".into() }).await.unwrap();
    assert_eq!(got.outcome, Outcome::Entry(Some(entry)));
    // A value larger than one MESSAGE carries goes in pieces, both ways.
    let big: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let stored = r.dpma.app_data(&Op::Set { app: "game".into(), scope: Scope::User, key: "save".into(), value: big.clone(), expect: None }).await.unwrap();
    assert!(matches!(stored.outcome, Outcome::Stored(e) if e.value == big));
    let hello = r.dpma.app_data(&Op::Hello { watch: vec!["game".into()] }).await.unwrap();
    assert_eq!(hello.outcome, Outcome::Hello);
    assert!(r.server.lock().unwrap().app_data.0.contains_key("watch/100"));

    // digium.config.update: fetch again with the same token.
    r.server.lock().unwrap().config = CONFIG.replace("789", "555").into_bytes();
    let push = r.server.lock().unwrap().event("digium.config.update", b"<config></config>");
    for m in push {
        r.to_phone.send(m).unwrap();
    }
    wait("second download", || (r.sink.applied.lock().unwrap().len() == 2).then_some(())).await;
    assert!(String::from_utf8_lossy(&r.sink.applied.lock().unwrap()[1]).contains("555"));
    assert_eq!(r.server.lock().unwrap().count("TokenRequest"), 2, "one refused, one accepted, none for the push");
    // The update itself and other server events are passed on.
    let push = r.server.lock().unwrap().event("digium.voicemail.play.location.change", b"{\"position\":3}");
    for m in push {
        r.to_phone.send(m).unwrap();
    }
    wait("push events", || {
        pushes(&mut seen);
        (seen.len() >= 2).then_some(())
    })
    .await;
    assert_eq!(seen[0].0, "digium.config.update");
    assert_eq!(seen[1], ("digium.voicemail.play.location.change".to_string(), "{\"position\":3}".to_string()));

    // The server forgets the session: the next request gets -398 and the
    // client handshakes again by itself and re-verifies its token.
    r.server.lock().unwrap().forget_sessions();
    let handshakes = r.server.lock().unwrap().count("Handshake");
    assert!(matches!(r.dpma.file("ringtone1").await, Err(DpmaError::Server { code: -398, .. })));
    wait("new handshake", || (r.server.lock().unwrap().count("Handshake") > handshakes).then_some(())).await;
    until(&r.store, "provisioned again", ProvisionPhase::Provisioned).await;
    assert_eq!(r.dpma.file("ringtone1").await.unwrap(), b"RIFFdata");

    // A session the server holds but refuses: its -398 comes encrypted
    // under a password we do not share, and is still understood.
    r.server.lock().unwrap().rekey_sessions();
    let handshakes = r.server.lock().unwrap().count("Handshake");
    assert!(matches!(r.dpma.file("ringtone1").await, Err(DpmaError::Server { code: -398, .. })));
    wait("handshake after an undecryptable error", || (r.server.lock().unwrap().count("Handshake") > handshakes).then_some(())).await;
    until(&r.store, "provisioned a third time", ProvisionPhase::Provisioned).await;

    // Reassignment: back to the handshake, token and user dropped.
    let push = r.server.lock().unwrap().reassign();
    for m in push {
        r.to_phone.send(m).unwrap();
    }
    until(&r.store, "awaiting a user after reassignment", ProvisionPhase::AwaitingUser).await;
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(c.get("config_server_token"), None);
}

#[tokio::test(flavor = "current_thread")]
async fn later_boot_md5_cache() {
    // First boot, MAC-keyed assignment: the user picks the server, and
    // nothing else is asked.
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.userlist_reqs = "disabled".into();
    s.md5 = Some("0123456789abcdef0123456789abcdef".into());
    let server = Arc::new(Mutex::new(s));
    let (files, sink) = (TempFiles::default(), Arc::new(MemSink::default()));
    let mdns = vec![DiscoveredServer {
        source: DiscoverySource::Mdns,
        name: "Digium Phone Config Server".into(),
        address: "sip:proxy@10.0.0.5:5060".into(),
    }];
    let r = rig_with(server.clone(), files.clone(), sink.clone());
    r.dpma.network(true, None, mdns.clone());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@10.0.0.5:5060".into()) }).await.unwrap();
    until(&r.store, "provisioned by MAC", ProvisionPhase::Provisioned).await;
    assert_eq!(r.store.get::<Provisioning>().server.as_ref().unwrap().source, DiscoverySource::Mdns, "a pick from the list keeps its source");
    // The server's digest is stored verbatim.
    assert_eq!(sink.md5.lock().unwrap().as_deref(), Some("0123456789abcdef0123456789abcdef"));
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 1);
    let identity = read_file(&files.identity_cert);
    drop(r);

    // Next boot: same files.  The saved server is used without
    // discovery, the token verifies, the digest matches: no download.
    let r = rig_with(server.clone(), files.clone(), sink.clone());
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned from the cache", ProvisionPhase::Provisioned).await;
    assert_eq!(r.store.get::<Provisioning>().server.as_ref().unwrap().source, DiscoverySource::Saved);
    assert_eq!(server.lock().unwrap().count("VerifyRequest"), 1);
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 1, "md5sum_match skips the download");
    assert_eq!(read_file(&files.identity_cert), identity, "the identity is minted once");
    drop(r);

    // A boot after the configuration changed: digest differs, download.
    server.lock().unwrap().md5 = Some("ffffffffffffffffffffffffffffffff".into());
    let r = rig_with(server.clone(), files.clone(), sink.clone());
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned after a change", ProvisionPhase::Provisioned).await;
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 2);
    assert_eq!(sink.md5.lock().unwrap().as_deref(), Some("ffffffffffffffffffffffffffffffff"));

    // Forgetting drops the saved server, token, and identity.
    r.store.send(Command::ForgetProvisioning).await.unwrap();
    until(&r.store, "awaiting a server after forgetting", ProvisionPhase::AwaitingServer).await;
    assert!(sink.configurator.lock().unwrap().is_none());
    assert!(!files.identity_cert.exists());
    // And skipping leaves the phone standalone.
    r.store.send(Command::SkipProvisioning).await.unwrap();
    assert_eq!(phase(&r.store), ProvisionPhase::Standalone);
    assert!(matches!(r.dpma.file("x").await, Err(DpmaError::NotAttached)));
}

#[tokio::test(flavor = "current_thread")]
async fn unreachable_server_retries() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    let r = rig_with(Arc::new(Mutex::new(s)), TempFiles::default(), Arc::new(MemSink::default()));
    *r.transport.down.lock().unwrap() = true;
    r.store.send(Command::StartProvisioning { server: Some("10.0.0.9:5060".into()) }).await.unwrap();
    r.dpma.network(true, None, Vec::new());
    wait("failure", || matches!(phase(&r.store), ProvisionPhase::Failed { .. }).then_some(())).await;
    assert_eq!(r.store.get::<Provisioning>().server.as_ref().unwrap().source, DiscoverySource::Manual);
    // What the user is told, and what happened, apart.
    let ProvisionPhase::Failed { reason, code, detail } = phase(&r.store) else { unreachable!() };
    assert_eq!(reason, "The server at \u{201c}10.0.0.9:5060\u{201d} did not answer. Check the address and the network cable.");
    assert_eq!((code, detail.as_str()), (None, "transport: unreachable"));
    // The server comes back; the retry timer provisions without help.
    *r.transport.down.lock().unwrap() = false;
    until(&r.store, "provisioned on retry", ProvisionPhase::Provisioned).await;
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(c.get("config_server_manual"), Some("1"));
    assert_eq!(c.get("config_server_type"), Some("digium"));
}

/// A server that leaves the Handshake unanswered takes the whole timeout
/// to give up on; the user cancels that, and the phone lets the server go
/// at once, waits for a choice, and does not try the server again by
/// itself.
#[tokio::test(flavor = "current_thread")]
async fn contacting_a_server_is_canceled() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.refuse_chain = true;
    let r = rig_timed(Arc::new(Mutex::new(s)), TempFiles::default(), Arc::new(MemSink::default()), Duration::from_secs(60));
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "awaiting a server", ProvisionPhase::AwaitingServer).await;
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test:5060".into()) }).await.unwrap();
    until(&r.store, "contacting", ProvisionPhase::Handshaking).await;
    let asked = std::time::Instant::now();
    r.store.send(Command::CancelProvisioning).await.unwrap();
    assert!(asked.elapsed() < Duration::from_secs(5), "the cancel waited for the timeout");
    assert_eq!(phase(&r.store), ProvisionPhase::AwaitingServer);
    // (The retry timer is a second.)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(phase(&r.store), ProvisionPhase::AwaitingServer);
    assert_eq!(r.server.lock().unwrap().handshake_certificates.len(), 1, "the server was contacted again");
    // A choice contacts it again.
    r.server.lock().unwrap().refuse_chain = false;
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test:5060".into()) }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
}

fn read_file(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Counts how often the firmware check asks what is running.
struct CountingInstaller(std::sync::atomic::AtomicUsize);
impl opendeskphone_dpma::firmware::FirmwareInstaller for CountingInstaller {
    fn running_image(&self) -> Result<opendeskphone_dpma::firmware::ImageChecksum, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("test".into())
    }
    fn install(&self, _: &mut dyn std::io::Read) -> Result<(), opendeskphone_dpma::firmware::InstallError> {
        unreachable!("the running image is unknown, so nothing may be installed")
    }
}
struct NoSource;
impl opendeskphone_dpma::firmware::FirmwareSource for NoSource {
    fn open(&self, _: &str, _: &str) -> Result<Box<dyn std::io::Read + Send>, opendeskphone_dpma::firmware::FetchError> {
        Err(opendeskphone_dpma::firmware::FetchError { status: None, message: "test".into() })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn provisioned_firmware_is_checked_unless_ignored() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    let server = Arc::new(Mutex::new(s));
    let r = rig_with(server.clone(), TempFiles::default(), Arc::new(MemSink::default()));
    let installer = Arc::new(CountingInstaller(Default::default()));
    r.dpma.set_firmware_updater(Arc::new(NoSource), installer.clone());
    let checks = || installer.0.load(std::sync::atomic::Ordering::SeqCst);

    // By default the provisioned firmware is respected: the entry is kept
    // and the check runs after the configuration is applied.
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    wait("the firmware check", || (checks() == 1).then_some(())).await;
    assert!(String::from_utf8_lossy(&r.sink.applied.lock().unwrap()[0]).contains("<firmware model=\"D60\""));
    assert_eq!(r.store.get::<Provisioning>().offered_firmware.as_deref(), Some("2_9_28"));

    // With the development setting on, the entry is dropped and nothing
    // is checked; the offer is still reported.
    r.dpma.set_ignore_provisioned_firmware(true);
    r.store.send(Command::Reprovision).await.unwrap();
    wait("second fetch", || (r.sink.applied.lock().unwrap().len() == 2).then_some(())).await;
    until(&r.store, "provisioned again", ProvisionPhase::Provisioned).await;
    assert!(!String::from_utf8_lossy(&r.sink.applied.lock().unwrap()[1]).contains("firmware"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(checks(), 1, "no check while ignoring");
    assert_eq!(r.store.get::<Provisioning>().offered_firmware.as_deref(), Some("2_9_28"));

    // And back.
    r.dpma.set_ignore_provisioned_firmware(false);
    r.store.send(Command::Reprovision).await.unwrap();
    wait("the check resumes", || (checks() == 2).then_some(())).await;
}

/// Stock and this firmware share the saved token in `configurator.xml`,
/// and the server ties a token to the spelling of the MAC it was last
/// presented with.  A token stock obtained must carry this client through
/// a whole boot (verify, then download), and must still be good for stock
/// afterward.
#[tokio::test(flavor = "current_thread")]
async fn a_token_saved_by_stock_stays_valid() {
    const STOCK_MAC: &[u8] = b"000FD3AABBCC";
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.userlist_reqs = "disabled".into();
    // Stock provisioned the phone and left its state in nvdata.
    let token = String::from_utf8(s.issue_token("100", STOCK_MAC)).unwrap();
    let server = Arc::new(Mutex::new(s));
    let (files, sink) = (TempFiles::default(), Arc::new(MemSink::default()));
    let mut c = Configurator::default();
    c.set("config_server_type", "digium");
    c.set("config_server_url", "sip:proxy@10.0.0.5:5060");
    c.set("config_server_user", "");
    c.set("config_server_token", &token);
    *sink.configurator.lock().unwrap() = Some(c.render());

    // Two boots of this firmware (a main-role restart and a reboot look
    // the same to the server: a new handshake, then the saved token).
    for boot in 1..=2 {
        let r = rig_with(server.clone(), files.clone(), sink.clone());
        r.dpma.network(true, None, Vec::new());
        until(&r.store, "provisioned with the saved token", ProvisionPhase::Provisioned).await;
        let s = server.lock().unwrap();
        assert_eq!(s.count("VerifyRequest"), boot);
        assert_eq!(s.count("ConfigRequest"), boot, "the download follows the verification with the same token");
        assert_eq!(s.count("TokenRequest"), 0, "the saved token was never refused");
        assert!(s.macs.iter().all(|m| m == STOCK_MAC), "every request spells the MAC as stock does: {:?}", s.macs);
        drop(s);
        drop(r);
    }
    assert_eq!(sink.applied.lock().unwrap().len(), 2);
    let saved = Configurator::parse(&sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(saved.get("config_server_token"), Some(token.as_str()), "the saved token is left as stock wrote it");

    // Stock boots next: VerifyRequest, then ConfigRequest, both with the
    // saved token and its own MAC spelling.
    let mut s = server.lock().unwrap();
    assert!(s.present_token(token.as_bytes(), STOCK_MAC), "stock's VerifyRequest");
    assert!(s.present_token(token.as_bytes(), STOCK_MAC), "stock's ConfigRequest");
}

/// A token of our own is spelled so that stock can use it.
#[tokio::test(flavor = "current_thread")]
async fn a_token_we_save_is_good_for_stock() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.userlist_reqs = "disabled".into();
    let server = Arc::new(Mutex::new(s));
    let (files, sink) = (TempFiles::default(), Arc::new(MemSink::default()));
    let r = rig_with(server.clone(), files.clone(), sink.clone());
    r.store.send(Command::StartProvisioning { server: Some("10.0.0.9:5060".into()) }).await.unwrap();
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    let saved = Configurator::parse(&sink.configurator.lock().unwrap().clone().unwrap());
    let token = saved.get("config_server_token").unwrap().to_string();
    assert!(token.starts_with("100/000FD3AABBCC/"), "{token}");
    let mut s = server.lock().unwrap();
    assert!(s.present_token(token.as_bytes(), b"000FD3AABBCC"), "stock's VerifyRequest");
    assert!(s.present_token(token.as_bytes(), b"000FD3AABBCC"), "stock's ConfigRequest");
}

/// The Handshake carries the certificate chain; a server that leaves that
/// unanswered gets the phone's certificate alone, and is remembered.
#[tokio::test(flavor = "current_thread")]
async fn handshake_sends_the_chain_and_falls_back_to_the_single_certificate() {
    let server = || {
        let mut s = FakeServer::new();
        s.config = CONFIG.as_bytes().to_vec();
        s.token_reqs = "mac".into();
        s.userlist_reqs = "disabled".into();
        s
    };

    // A server that reads the chain form.
    let r = rig_with(Arc::new(Mutex::new(server())), TempFiles::default(), Arc::new(MemSink::default()));
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned with the chain form", ProvisionPhase::Provisioned).await;
    assert_eq!(r.server.lock().unwrap().handshake_certificates, vec![3]);
    drop(r);

    // A server that does not.
    let mut s = server();
    s.refuse_chain = true;
    let r = rig_with(Arc::new(Mutex::new(s)), TempFiles::default(), Arc::new(MemSink::default()));
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned with the single certificate", ProvisionPhase::Provisioned).await;
    assert_eq!(r.server.lock().unwrap().handshake_certificates, vec![3, 1]);
    assert_eq!(r.sink.applied.lock().unwrap().len(), 1);
    // The next handshake with that server goes straight to the form it
    // answered.
    r.store.send(Command::Reprovision).await.unwrap();
    wait("second fetch", || (r.sink.applied.lock().unwrap().len() == 2).then_some(())).await;
    assert_eq!(r.server.lock().unwrap().handshake_certificates, vec![3, 1, 1]);
}

/// What the server required at the handshake still holds on a pass that
/// reuses the session: after `digium.config.update` with `delete_token`,
/// the TokenRequest carries the password again.
#[tokio::test(flavor = "current_thread")]
async fn requirements_are_kept_for_a_token_request_without_a_handshake() {
    let r = rig();
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "awaiting a user", ProvisionPhase::AwaitingUser).await;
    r.store.send(Command::SelectUser { id: "100".into(), pin: "1234".into() }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    assert_eq!(r.server.lock().unwrap().token_requests, vec![("100".to_string(), "1234".to_string(), String::new())]);

    let handshakes = r.server.lock().unwrap().count("Handshake");
    r.server.lock().unwrap().config = CONFIG.replace("789", "555").into_bytes();
    let push = r.server.lock().unwrap().event(
        "digium.config.update",
        b"<config>\t<section id=\"all\"/>\t<option id=\"delete_token\" value=\"true\"/></config>",
    );
    for m in push {
        r.to_phone.send(m).unwrap();
    }
    wait("download with a new token", || (r.sink.applied.lock().unwrap().len() == 2).then_some(())).await;
    until(&r.store, "provisioned again", ProvisionPhase::Provisioned).await;
    let s = r.server.lock().unwrap();
    assert_eq!(s.count("Handshake"), handshakes, "the session was reused");
    assert_eq!(s.token_requests.len(), 2);
    assert_eq!(s.token_requests[1], ("100".to_string(), "1234".to_string(), String::new()), "user and password as the server required");
    drop(s);
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert!(c.get("config_server_token").is_some(), "the new token is saved");
}

/// A configuration that turns "ignore provisioned firmware" on and offers
/// firmware at once: the setting applies to that very configuration.
#[tokio::test(flavor = "current_thread")]
async fn ignore_setting_in_the_fetched_configuration_covers_its_own_firmware() {
    let with_setting = CONFIG.replace("<firmwares>", "<setting id=\"opendeskphone_ignore_provisioned_firmware\" value=\"1\" />\n<firmwares>");
    let mut s = FakeServer::new();
    s.config = with_setting.into_bytes();
    s.token_reqs = "mac".into();
    let server = Arc::new(Mutex::new(s));
    let sink = Arc::new(MemSink { reads_ignore_setting: true, ..Default::default() });
    let r = rig_with(server.clone(), TempFiles::default(), sink);
    let installer = Arc::new(CountingInstaller(Default::default()));
    r.dpma.set_firmware_updater(Arc::new(NoSource), installer.clone());
    let checks = || installer.0.load(std::sync::atomic::Ordering::SeqCst);

    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    let applied = String::from_utf8(r.sink.applied.lock().unwrap()[0].clone()).unwrap();
    assert!(applied.contains("opendeskphone_ignore_provisioned_firmware"));
    assert!(!applied.contains("<firmware"), "the entries are dropped from the saved configuration: {applied}");
    assert_eq!(r.store.get::<Provisioning>().offered_firmware.as_deref(), Some("2_9_28"), "the offer is still reported");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(checks(), 0, "nothing is compared or installed");

    // The server takes the setting back: the same pass acts on the
    // firmware again.
    server.lock().unwrap().config = CONFIG.as_bytes().to_vec();
    let push = server.lock().unwrap().event("digium.config.update", b"<config></config>");
    for m in push {
        r.to_phone.send(m).unwrap();
    }
    wait("the firmware check", || (checks() == 1).then_some(())).await;
    assert!(String::from_utf8_lossy(&r.sink.applied.lock().unwrap()[1]).contains("<firmware model=\"D60\""));
}

/// A setting changed on the phone reaches a firmware check that has not
/// started: the integrator reports it through the handle.
#[tokio::test(flavor = "current_thread")]
async fn ignore_setting_changed_on_the_phone_is_followed() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.md5 = Some("0123456789abcdef0123456789abcdef".into());
    let server = Arc::new(Mutex::new(s));
    // A sink that keeps the fetched configuration, so that a boot with a
    // matching digest has something to check.
    struct Keeping(MemSink);
    impl opendeskphone_dpma::ConfigSink for Keeping {
        fn configurator(&self) -> Option<Vec<u8>> {
            self.0.configurator()
        }
        fn write_configurator(&self, xml: &[u8]) -> Result<(), String> {
            self.0.write_configurator(xml)
        }
        fn cached_md5(&self) -> Option<String> {
            self.0.cached_md5()
        }
        fn apply(&self, fetched_config: &[u8], md5: &str) -> Result<(), String> {
            self.0.apply(fetched_config, md5)
        }
        fn fetched_config(&self) -> Option<Vec<u8>> {
            self.0.applied.lock().unwrap().last().cloned()
        }
        fn forget(&self) -> Result<(), String> {
            self.0.forget()
        }
    }
    let store = Store::new(Model::D60);
    let (to_phone, incoming) = mpsc::unbounded_channel();
    let transport = Arc::new(Mem { server: server.clone(), to_phone, down: Mutex::new(false) });
    let mut cfg = DpmaConfig::new("00:0F:D3:AA:BB:CC", "Digium D60 3_0_0");
    cfg.timeout = Duration::from_millis(500);
    cfg.ping_secs = 0;
    let sink = Arc::new(Keeping(MemSink::default()));
    let files = TempFiles::default();
    let dpma = opendeskphone_dpma::start(store.clone(), cfg, transport, incoming, files.files(), sink.clone());
    let installer = Arc::new(CountingInstaller(Default::default()));
    dpma.set_firmware_updater(Arc::new(NoSource), installer.clone());
    let checks = || installer.0.load(std::sync::atomic::Ordering::SeqCst);

    dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&store, "provisioned", ProvisionPhase::Provisioned).await;
    wait("the first check", || (checks() == 1).then_some(())).await;

    // The user turns the setting on; the next pass finds the digest
    // unchanged, downloads nothing, and leaves the firmware alone.
    dpma.set_ignore_provisioned_firmware(true);
    store.send(Command::Reprovision).await.unwrap();
    wait("verified", || (server.lock().unwrap().count("VerifyRequest") == 1).then_some(())).await;
    until(&store, "provisioned again", ProvisionPhase::Provisioned).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 1, "the digest matched: no download");
    assert_eq!(checks(), 1, "no check while ignoring");
}

/// Serves HTTP URLs from a table.
struct MapSource(std::collections::HashMap<String, Vec<u8>>, Mutex<Vec<String>>);
impl opendeskphone_dpma::firmware::FirmwareSource for MapSource {
    fn open(&self, url: &str, _: &str) -> Result<Box<dyn std::io::Read + Send>, opendeskphone_dpma::firmware::FetchError> {
        self.1.lock().unwrap().push(url.to_string());
        match self.0.get(url) {
            Some(data) => Ok(Box::new(std::io::Cursor::new(data.clone()))),
            None => Err(opendeskphone_dpma::firmware::FetchError { status: Some(404), message: "Not Found".into() }),
        }
    }
}

fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// After a configuration is in effect, the files it names are fetched and
/// kept where stock keeps them; a matching digest skips the download, on
/// the later-boot path too (VerifyResponse with the configuration's
/// digest: no ConfigRequest, and the files are still looked after).
#[tokio::test(flavor = "current_thread")]
async fn provisioned_files_are_fetched_checked_and_pruned() {
    use common::MemFiles;
    use opendeskphone_dpma::resources::{Kind, PushedApp, Resource, Wanted, RINGTONE_BUDGET};

    const CONTACTS: &[u8] = b"<contacts group_name=\"Team\"><contact id=\"c1\" first_name=\"Ann\"/></contacts>";
    const BLF: &[u8] = b"<config><smart_blf><blf_items><item location=\"side\" index=\"0\" contact_id=\"c1\"/></blf_items></smart_blf></config>";
    let chimes = vec![7u8; 4000];
    let web_list = b"<contacts group_name=\"Web\"/>".to_vec();

    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.userlist_reqs = "disabled".into();
    s.md5 = Some("0123456789abcdef0123456789abcdef".into());
    s.files.insert("contacts.xml".into(), CONTACTS.to_vec());
    s.files.insert("blf.xml".into(), BLF.to_vec());
    s.files.insert("chimes.sln".into(), chimes.clone());
    let server = Arc::new(Mutex::new(s));

    let files = Arc::new(MemFiles::default());
    // Left by earlier passes or by the user.
    files.put("contact_lists/gone", b"old");
    files.put("md5s/gone", b"00000000000000000000000000000000");
    files.put("contact_lists/mine.xml", b"the user's own list");
    files.put("contact_lists/local_contacts.xml", b"local");
    files.put("contact_lists/serialized_contacts.xml", b"stock's merge");
    files.put("md5s/serialized_contacts", b"11111111111111111111111111111111");
    files.put("contact_lists/3", b"stale but present");
    files.put("md5s/3", b"22222222222222222222222222222222");
    files.put("ringtones/Old.sln16", b"old tone");
    files.put("md5s/Old", b"33333333333333333333333333333333");
    files.put("ringtones/user_upload_1.sln16", b"uploaded");
    files.put("datasets.xml", b"old datasets");
    files.put("md5s/datasets", b"44444444444444444444444444444444");

    let res = |kind, id: &str, url: &str, md5: Option<&str>| Resource { kind, id: id.into(), url: url.into(), md5: md5.map(str::to_string) };
    let wanted = Wanted {
        resources: vec![
            res(Kind::ContactList, "0", "file://contacts.xml", None),
            res(Kind::ContactList, "web.xml", "http://pbx.test/lists/web.xml", None),
            // The server has no such file: the copy on disk stays.
            res(Kind::ContactList, "3", "file://missing.xml", None),
            // Not usable, as on stock: no id, or no URL.
            res(Kind::ContactList, "", "file://contacts.xml", None),
            res(Kind::ContactList, "9", "", None),
            res(Kind::Ringtone, "Chimes", "file://chimes.sln", Some(&md5_hex(&chimes).to_uppercase())),
            res(Kind::Ringtone, "Huge", "http://pbx.test/huge.sln", None),
            res(Kind::BlfItems, "", "file://blf.xml", None),
        ],
        apps: vec![PushedApp { id: "queues".into(), url: "file://queues.zip".into() }],
    };
    let sink = Arc::new(MemSink { wanted: Mutex::new(wanted), files: Mutex::new(Some(files.clone())), ..Default::default() });
    let client = TempFiles::default();
    let r = rig_with(server.clone(), client.clone(), sink.clone());
    let http = Arc::new(MapSource(
        [
            ("http://pbx.test/lists/web.xml".to_string(), web_list.clone()),
            ("http://pbx.test/huge.sln".to_string(), vec![0u8; RINGTONE_BUDGET + 1]),
        ]
        .into(),
        Mutex::new(Vec::new()),
    ));
    r.dpma.set_http_source(http.clone());
    let changes = || sink.resource_changes.load(std::sync::atomic::Ordering::SeqCst);

    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    wait("the files", || (changes() == 1).then_some(())).await;

    // Where stock keeps them, with the digest of what was downloaded.
    assert_eq!(files.get("contact_lists/0").as_deref(), Some(CONTACTS));
    assert_eq!(files.get("md5s/0"), Some(md5_hex(CONTACTS).into_bytes()));
    assert_eq!(files.get("contact_lists/web%2Exml"), Some(web_list.clone()));
    assert_eq!(files.get("md5s/web%2Exml"), Some(md5_hex(&web_list).into_bytes()));
    assert_eq!(files.get("ringtones/Chimes.sln16"), Some(chimes.clone()));
    assert_eq!(files.get("md5s/Chimes"), Some(md5_hex(&chimes).into_bytes()));
    assert_eq!(files.get("blf_items.xml").as_deref(), Some(BLF));
    assert_eq!(files.get("md5s/blf_items"), Some(md5_hex(BLF).into_bytes()));
    // A failed download leaves what was there.
    assert_eq!(files.get("contact_lists/3").as_deref(), Some(&b"stale but present"[..]));
    // Over the ringtone budget: not kept.
    assert_eq!(files.get("ringtones/Huge.sln16"), None);
    // No longer named, and downloaded by an earlier pass: removed with
    // their digests.
    for gone in ["contact_lists/gone", "md5s/gone", "ringtones/Old.sln16", "md5s/Old", "datasets.xml", "md5s/datasets"] {
        assert_eq!(files.get(gone), None, "{gone}");
    }
    // Never touched.
    assert_eq!(files.get("contact_lists/mine.xml").as_deref(), Some(&b"the user's own list"[..]));
    assert_eq!(files.get("contact_lists/local_contacts.xml").as_deref(), Some(&b"local"[..]));
    assert_eq!(files.get("contact_lists/serialized_contacts.xml").as_deref(), Some(&b"stock's merge"[..]));
    assert_eq!(files.get("ringtones/user_upload_1.sln16").as_deref(), Some(&b"uploaded"[..]));
    // The pushed app was not asked for.
    let asked = server.lock().unwrap().file_requests.clone();
    assert_eq!(asked, vec!["contacts.xml", "missing.xml", "chimes.sln", "blf.xml"]);
    assert_eq!(*http.1.lock().unwrap(), vec!["http://pbx.test/lists/web.xml", "http://pbx.test/huge.sln"]);
    drop(r);

    // The next boot: the token verifies, the VerifyResponse carries the
    // digest of the configuration on the phone, and nothing is downloaded
    // for the configuration.  Its files are looked after all the same: an
    // entry without a digest is asked for again, one whose digest matches
    // is not, and unchanged content is not written again.
    let before = changes();
    let r = rig_with(server.clone(), client.clone(), sink.clone());
    r.dpma.set_http_source(http.clone());
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned from the cache", ProvisionPhase::Provisioned).await;
    wait("the second pass over the files", || (server.lock().unwrap().file_requests.len() == 7).then_some(())).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    {
        let s = server.lock().unwrap();
        assert_eq!(s.count("VerifyRequest"), 1);
        assert_eq!(s.count("ConfigRequest"), 1, "X-Digium-AppServer-MD5 on the VerifyResponse matched: no download");
        assert_eq!(s.file_requests[4..], ["contacts.xml", "missing.xml", "blf.xml"], "the ringtone's digest matched");
    }
    assert_eq!(changes(), before, "nothing on disk changed");

    // The server's contact list changes: the next pass replaces the file
    // and says so.
    server.lock().unwrap().files.insert("contacts.xml".into(), b"<contacts group_name=\"Team\"/>".to_vec());
    r.store.send(Command::Reprovision).await.unwrap();
    wait("the new contact list", || (files.get("contact_lists/0").as_deref() == Some(&b"<contacts group_name=\"Team\"/>"[..])).then_some(())).await;
    wait("the change is reported", || (changes() == before + 1).then_some(())).await;
    assert_eq!(files.get("md5s/0"), Some(md5_hex(b"<contacts group_name=\"Team\"/>").into_bytes()));
}

/// A phone with no saved server lists what discovery finds and waits for
/// its user, whether that is one server or several (the state after a
/// factory reset, which removes `configurator.xml`).  Discovery inputs that
/// are reported again unchanged do not disturb a phone that waits.
#[tokio::test(flavor = "current_thread")]
async fn discovery_inputs_are_followed() {
    let r = rig();
    let mdns = |addr: &str| vec![DiscoveredServer { source: DiscoverySource::Mdns, name: "Digium Phone Config Server".into(), address: addr.into() }];
    let listed = |r: &Rig| r.store.get::<Provisioning>().discovered.iter().map(|d| (d.source, d.address.clone())).collect::<Vec<_>>();
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "awaiting a server", ProvisionPhase::AwaitingServer).await;

    // One server appears over mDNS while the phone waits: it is listed,
    // and the phone goes on waiting.
    r.dpma.network(true, None, mdns("10.0.0.5:5060"));
    wait("the announced server is listed", || (listed(&r) == [(DiscoverySource::Mdns, "10.0.0.5:5060".to_string())]).then_some(())).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(phase(&r.store), ProvisionPhase::AwaitingServer);
    assert_eq!(r.server.lock().unwrap().count("Handshake"), 0, "nothing is sent to a server nobody picked");
    assert_eq!(r.store.get::<Provisioning>().server, None);

    // DHCP option 66 arrives while the address stays: listed first, and
    // still nothing is picked.
    r.dpma.network(true, Some("sip:proxy@pbx.test:5060".into()), mdns("10.0.0.5:5060"));
    wait("option 66 is listed", || (listed(&r).first().map(|d| d.0) == Some(DiscoverySource::DhcpOption66)).then_some(())).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(phase(&r.store), ProvisionPhase::AwaitingServer);
    assert_eq!(r.server.lock().unwrap().count("Handshake"), 0);

    // The user picks the announced server.
    r.store.send(Command::StartProvisioning { server: Some("10.0.0.5:5060".into()) }).await.unwrap();
    until(&r.store, "awaiting a user", ProvisionPhase::AwaitingUser).await;
    assert_eq!(r.store.get::<Provisioning>().server.as_ref().unwrap().source, DiscoverySource::Mdns);
    assert_eq!(r.server.lock().unwrap().count("Handshake"), 1);

    // The same inputs again: nothing happens.
    r.dpma.network(true, Some("sip:proxy@pbx.test:5060".into()), mdns("10.0.0.5:5060"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(r.server.lock().unwrap().count("Handshake"), 1);
    assert_eq!(phase(&r.store), ProvisionPhase::AwaitingUser);

    r.store.send(Command::SelectUser { id: "100".into(), pin: "1234".into() }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;

    // Attached: new discovery results are listed, and the session stays.
    let handshakes = r.server.lock().unwrap().count("Handshake");
    r.dpma.network(true, Some("sip:proxy@pbx.test:5060".into()), mdns("10.0.0.6:5060"));
    wait("the list follows", || {
        r.store.get::<Provisioning>().discovered.iter().any(|d| d.address == "10.0.0.6:5060").then_some(())
    })
    .await;
    assert_eq!(r.server.lock().unwrap().count("Handshake"), handshakes);
    assert_eq!(phase(&r.store), ProvisionPhase::Provisioned);
}

/// A server that sends the MD5 of the configuration document on both the
/// ConfigResponse and the VerifyResponse (Open DPMA; dpma-rs from 0.3.1
/// on): a boot with an unchanged document downloads nothing, and a changed
/// document is downloaded.
#[tokio::test(flavor = "current_thread")]
async fn document_digest_on_verify_skips_the_download() {
    let mut s = FakeServer::new();
    s.config = CONFIG.as_bytes().to_vec();
    s.token_reqs = "mac".into();
    s.userlist_reqs = "disabled".into();
    s.md5_of_document = true;
    let server = Arc::new(Mutex::new(s));
    let (files, sink) = (TempFiles::default(), Arc::new(MemSink::default()));
    let boot = || rig_with(server.clone(), files.clone(), sink.clone());

    let r = boot();
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "provisioned", ProvisionPhase::Provisioned).await;
    assert_eq!(sink.md5.lock().unwrap().as_deref(), Some(md5_hex(CONFIG.as_bytes()).as_str()), "the server's digest is the cache");
    drop(r);

    let r = boot();
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned from the cache", ProvisionPhase::Provisioned).await;
    assert_eq!(server.lock().unwrap().count("VerifyRequest"), 1);
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 1, "the digests match: no download");
    assert_eq!(sink.applied.lock().unwrap().len(), 1);
    drop(r);

    let changed = CONFIG.replace("789", "555");
    server.lock().unwrap().config = changed.clone().into_bytes();
    let r = boot();
    r.dpma.network(true, None, Vec::new());
    until(&r.store, "provisioned after a change", ProvisionPhase::Provisioned).await;
    assert_eq!(server.lock().unwrap().count("ConfigRequest"), 2);
    assert_eq!(sink.applied.lock().unwrap().len(), 2);
    assert_eq!(sink.md5.lock().unwrap().as_deref(), Some(md5_hex(changed.as_bytes()).as_str()));
}

fn selection(store: &Store) -> Option<Result<(), String>> {
    store.get::<Provisioning>().selection.clone()
}

/// A login from the phone's menus or an app while the phone has a user
/// (hot desking): the PBX is asked about the new user, a refusal says why
/// and leaves the phone with the user it had, and a logout leaves the
/// phone without one.
#[tokio::test(flavor = "current_thread")]
async fn a_refused_login_brings_the_previous_assignment_back() {
    let r = rig();
    r.server.lock().unwrap().users.push(("200".into(), "Other".into(), "4321".into()));
    r.dpma.network(true, Some("sip:proxy@pbx.test".into()), Vec::new());
    r.store.send(Command::StartProvisioning { server: Some("sip:proxy@pbx.test".into()) }).await.unwrap();
    until(&r.store, "awaiting a user", ProvisionPhase::AwaitingUser).await;
    assert_eq!(selection(&r.store), None);
    r.store.send(Command::SelectUser { id: "100".into(), pin: "1234".into() }).await.unwrap();
    wait("the first login ended", || selection(&r.store)).await.unwrap();
    assert_eq!(phase(&r.store), ProvisionPhase::Provisioned);

    // A wrong PIN for another user: the PBX is asked, and refuses.
    r.store.send(Command::SelectUser { id: "200".into(), pin: "0000".into() }).await.unwrap();
    let refused = wait("the login was refused", || selection(&r.store)).await.unwrap_err();
    assert!(refused.contains("-199"), "{refused}");
    assert_eq!(r.server.lock().unwrap().token_requests.last().unwrap().0, "200");
    // The phone is the first user's again, attached, with nothing saved
    // for the user it was refused as.
    until(&r.store, "provisioned again", ProvisionPhase::Provisioned).await;
    let p = r.store.get::<Provisioning>();
    assert!(p.attached);
    assert_eq!(p.assigned_user, "100");
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(c.get("config_server_user"), Some("100"));
    assert!(c.get("config_server_token").unwrap().starts_with("100/"));
    // The refusal is still what the state says of the login.
    assert!(selection(&r.store).unwrap().is_err());

    // The right PIN: the phone becomes the other user's.
    r.store.send(Command::SelectUser { id: "200".into(), pin: "4321".into() }).await.unwrap();
    wait("the login ended", || selection(&r.store)).await.unwrap();
    let p = r.store.get::<Provisioning>();
    assert_eq!((p.phase.clone(), p.assigned_user.as_str(), p.attached), (ProvisionPhase::Provisioned, "200", true));
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!(c.get("config_server_user"), Some("200"));
    assert!(c.get("config_server_token").unwrap().starts_with("200/"));

    // A logout: this PBX has no configuration for a phone without a user,
    // so the phone waits for one, and keeps nothing of the user it had.
    r.store.send(Command::SelectUser { id: String::new(), pin: String::new() }).await.unwrap();
    wait("the logout ended", || selection(&r.store)).await.unwrap();
    let p = r.store.get::<Provisioning>();
    assert_eq!((p.phase.clone(), p.assigned_user.as_str()), (ProvisionPhase::AwaitingUser, ""));
    let c = Configurator::parse(&r.sink.configurator.lock().unwrap().clone().unwrap());
    assert_eq!((c.get("config_server_user"), c.get("config_server_token")), (None, None));
}
