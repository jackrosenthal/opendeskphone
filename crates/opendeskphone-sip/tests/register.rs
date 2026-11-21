//! Registration with digest authentication and failover to the alternate
//! host, the MWI subscription and the one presence subscription of a
//! watched contact (with pickup of its ringing call), and out-of-dialog
//! MESSAGE in both directions, against a hand-written UDP server.

mod common;

use common::{base_config, phone_with, wait, Msg, Server};
use opendeskphone_sip::{AccountConfig, BlfConfig, HostConfig, OutMessage};
use opendeskphone_state::{
    AccountId, Accounts, ActiveHost, Blink, BlfState, Command, Keys, LampColor, LampState, Mwi, Presence,
    PresenceStatus, Registration,
};

/// Presence documents as Asterisk sent them (see `bodies.rs`).
const RINGING: &str = include_str!("data/presence/ringing.xml");
const READY_AWAY: &str = include_str!("data/presence/ready-away-lunch.xml");

fn notify(sub: &Msg, server: std::net::SocketAddr, event: &str, ctype: &str, body: &str, seq: u32) -> String {
    let contact = sub.header("Contact").unwrap().trim_matches(|c| c == '<' || c == '>').to_string();
    format!(
        "NOTIFY {contact} SIP/2.0\r\nVia: SIP/2.0/UDP {server};branch=z9hG4bKsrv{seq}{}\r\nMax-Forwards: 70\r\n\
         From: {};tag=srv\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {seq} NOTIFY\r\nEvent: {event}\r\n\
         Subscription-State: active;expires=600\r\nContact: <sip:{server}>\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n{body}",
        sub.header("Call-ID").unwrap().replace('@', ""),
        sub.header("To").unwrap(),
        sub.header("From").unwrap(),
        sub.header("Call-ID").unwrap(),
        body.len()
    )
}

#[tokio::test(flavor = "current_thread")]
async fn register_failover_subscribe_message() {
    // The primary refuses service; the alternate challenges and accepts.
    let bad = Server::start(Box::new(|m: &Msg| vec![m.reply(503, "Service Unavailable", &[], "")])).await;
    let good_addr = std::sync::Arc::new(std::sync::Mutex::new(None::<std::net::SocketAddr>));
    let ga = good_addr.clone();
    let good = Server::start(Box::new(move |m: &Msg| {
        let me = ga.lock().unwrap().expect("address set");
        match m.method() {
            "REGISTER" => match m.header("Authorization") {
                // The digest itself is the stack's; the server only needs
                // to see that credentials for the challenge came back.
                Some(auth) if auth.contains("nonce=\"abc123\"") && auth.contains("response=") => {
                    vec![m.reply(200, "OK", &[("Contact", m.header("Contact").unwrap().to_string())], "")]
                }
                _ => vec![m.reply(
                    401,
                    "Unauthorized",
                    &[("WWW-Authenticate", "Digest realm=\"test\", nonce=\"abc123\", qop=\"auth\"".into())],
                    "",
                )],
            },
            "SUBSCRIBE" => {
                let event = m.header("Event").unwrap_or("").to_string();
                let ok = m.reply(200, "OK", &[("Expires", "600".into()), ("Contact", format!("<sip:{me}>"))], "");
                if event == "message-summary" {
                    let n = notify(m, me, "message-summary", "application/simple-message-summary", "Messages-Waiting: yes\r\nVoice-Message: 2/8 (0/0)\r\n", 1);
                    vec![ok, n]
                } else {
                    // The first NOTIFY may overtake the 200: send it first.
                    vec![notify(m, me, &event, "application/pidf+xml", RINGING, 1), ok]
                }
            }
            "ACK" => vec![],
            "INVITE" => vec![m.reply(404, "Not Found", &[], "")],
            _ => vec![m.reply(200, "OK", &[], "")],
        }
    }))
    .await;
    *good_addr.lock().unwrap() = Some(good.addr);

    let mut cfg = base_config();
    let mut a = AccountConfig::new(AccountId(0), "100", "secret", "127.0.0.1");
    a.primary.port = bad.addr.port();
    a.primary.num_retries = 1;
    a.primary.retry_secs = 1;
    let mut alt = HostConfig::new("127.0.0.1");
    alt.port = good.addr.port();
    a.alternate = Some(alt);
    a.auth_user = "auth100".into();
    cfg.accounts.push(a);
    cfg.blf.push(BlfConfig {
        id: "c101".into(),
        label: "Bob".into(),
        number: "101".into(),
        subscribe: "auto_hint_101".into(),
        account: AccountId(0),
        pickup_ringing: true,
        location: opendeskphone_sip::BlfLocation::Side,
        index: 0,
        paging: true,
        blank: false,
    });
    let p = phone_with("100", cfg, 4000).await;
    let mut inbox = p.handle.messages();

    wait("registration on the alternate", || {
        matches!(
            p.store.get::<Accounts>().get(AccountId(0)).map(|a| a.registration.clone()),
            Some(Registration::Registered { host: ActiveHost::Alternate, .. })
        )
        .then_some(())
    })
    .await;
    assert!(!bad.requests("REGISTER").is_empty());
    let regs = good.requests("REGISTER");
    assert!(regs.len() >= 2, "challenge, then credentials");
    assert_eq!(regs[0].header("User-Agent"), Some("Digium D60 3_1_0"));
    let auth = regs[1].header("Authorization").expect("credentials");
    assert!(auth.contains("username=\"auth100\""), "{auth}");
    assert!(regs[1].header("Contact").unwrap().contains(&format!("sip:100@127.0.0.1:{}", p.handle.local_port())));

    // MWI from the message-summary NOTIFY.
    wait("MWI", || p.store.get::<Mwi>().accounts.first().filter(|m| m.new == 2 && m.old == 8 && m.waiting).map(|_| ())).await;
    // New voicemail: slow red, as stock's base configuration has it.
    assert_eq!(p.store.get::<Keys>().message_lamp, LampState::blinking(LampColor::Red, Blink::Slow));
    // BLF from the presence NOTIFY: the line rings; the user's status is
    // the Digium element's, not the `on-the-phone` activity.
    wait("BLF", || p.store.get::<Presence>().get("c101").filter(|e| e.state == BlfState::Ringing).map(|_| ())).await;
    let entry = p.store.get::<Presence>().get("c101").cloned().unwrap();
    assert_eq!((entry.status, entry.status_text.as_str()), (PresenceStatus::Available, ""));
    assert!(entry.subscribed && entry.pickup_ringing);
    let keys = p.store.get::<Keys>();
    // A D60 has no rapid-dial strip: the item is on the free line key.
    assert_eq!(keys.line_keys[1].binding, opendeskphone_state::LineKeyBinding::Blf("c101".into()));
    assert_eq!(keys.line_keys[1].lamp, LampState::blinking(LampColor::Green, Blink::Fast));
    assert!(keys.rapid_dial.is_empty());
    // The contact has one subscription: `presence`, with stock's Accept
    // and expiry.  Nothing subscribes to `dialog`.
    let subs = good.requests("SUBSCRIBE");
    let events = |uri: &str| -> Vec<String> {
        let mut dialogs: Vec<(String, String)> = subs
            .iter()
            .filter(|s| s.start.starts_with(uri))
            .map(|s| (s.header("Call-ID").unwrap().to_string(), s.header("Event").unwrap().to_string()))
            .collect();
        dialogs.sort();
        dialogs.dedup();
        dialogs.into_iter().map(|d| d.1).collect()
    };
    assert_eq!(events("SUBSCRIBE sip:auto_hint_101@127.0.0.1"), ["presence"]);
    assert_eq!(events("SUBSCRIBE sip:100@127.0.0.1"), ["message-summary"]);
    assert!(subs.iter().all(|s| s.header("Event") != Some("dialog")));
    let sub = subs.iter().find(|s| s.header("Event") == Some("presence")).unwrap();
    assert_eq!(sub.header("Accept"), Some("application/pidf+xml, application/xpidf+xml"));
    assert_eq!(sub.header("Expires"), Some("600"));

    // A press on the ringing contact's key is a pickup, since the contact
    // allows it: a call to the contact's number (not to the subscription
    // target) marked with stock's header, and no Replaces.
    assert!(p.store.send(Command::Pickup { blf_entry: "c101".into() }).await.is_ok());
    let invite = wait("the pickup INVITE", || good.requests("INVITE").pop()).await;
    assert!(invite.start.starts_with("INVITE sip:101@127.0.0.1"), "{}", invite.start);
    assert_eq!(invite.header("X-Digium-Call-Feature"), Some("feature_pickup"));
    assert_eq!(invite.header("Replaces"), None);

    // A later NOTIFY belongs to the established dialog (it carries the
    // tags both sides now know) and updates the same entry: idle, and the
    // user away with a message.
    let idle = notify(sub, good.addr, "presence", "application/pidf+xml", READY_AWAY, 2);
    good.socket.send_to(idle.as_bytes(), format!("127.0.0.1:{}", p.handle.local_port())).await.unwrap();
    wait("BLF idle", || p.store.get::<Presence>().get("c101").filter(|e| e.state == BlfState::Idle).map(|_| ())).await;
    let entry = p.store.get::<Presence>().get("c101").cloned().unwrap();
    assert_eq!((entry.status, entry.status_text.as_str()), (PresenceStatus::Away, "Back at 1"));
    // An idle contact lights nothing (stock's default behavior).
    assert_eq!(p.store.get::<Keys>().line_keys[1].lamp, LampState::OFF);

    // MESSAGE out (as DPMA sends it: from the MAC at the phone's address).
    let code = p
        .handle
        .send_message(OutMessage {
            account: None,
            to: format!("sip:proxy@{}", good.addr),
            from_user: Some("000fd3aabbcc".into()),
            headers: vec![("X-Digium-AppServer-RequestType".into(), "Handshake".into())],
            content_type: String::new(),
            body: b"hello".to_vec(),
        })
        .await
        .expect("MESSAGE answered");
    assert_eq!(code, 200);
    let sent = good.requests("MESSAGE").pop().expect("server saw the MESSAGE");
    assert_eq!(sent.header("X-Digium-AppServer-RequestType"), Some("Handshake"));
    assert_eq!(sent.body, "hello");
    assert!(sent.header("From").unwrap().contains("sip:000fd3aabbcc@127.0.0.1"));

    // MESSAGE in: delivered to the listener and answered 200.
    let phone_addr = format!("127.0.0.1:{}", p.handle.local_port());
    let incoming = format!(
        "MESSAGE sip:000fd3aabbcc@{phone_addr} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bKin1\r\nMax-Forwards: 70\r\n\
         From: <sip:proxy@{}>;tag=s1\r\nTo: <sip:000fd3aabbcc@{phone_addr}>\r\nCall-ID: in1@srv\r\nCSeq: 7 MESSAGE\r\n\
         X-Digium-AppServer-ResponseType: HandshakeResponse\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nbody",
        good.addr, good.addr
    );
    good.socket.send_to(incoming.as_bytes(), &phone_addr).await.unwrap();
    let got = tokio::time::timeout(std::time::Duration::from_secs(5), inbox.recv()).await.expect("in time").expect("message");
    assert_eq!(got.body, b"body");
    assert!(got.headers.iter().any(|(k, v)| k == "X-Digium-AppServer-ResponseType" && v == "HandshakeResponse"));
    wait("200 to the incoming MESSAGE", || {
        good.log.lock().unwrap().iter().any(|m| m.start.starts_with("SIP/2.0 200") && m.header("Call-ID") == Some("in1@srv")).then_some(())
    })
    .await;

    // Shutdown un-registers.
    p.handle.shutdown().await;
    wait("un-REGISTER", || good.requests("REGISTER").iter().any(|r| r.header("Expires") == Some("0")).then_some(())).await;
}
