//! The provisioning state machine (plan section 9): discover, handshake,
//! verify or obtain a token, fetch and apply the configuration and the
//! files it names, then stay attached for pushes.

use crate::link::{body_of, DpmaError, Link};
use crate::resources;
use crate::xml::{self, Configurator};
use crate::{ClientFiles, ConfigSink, DpmaConfig, FirmwareCtl, Input};
use opendeskphone_config::nvdata::{write_atomic, write_atomic_private};
use std::sync::atomic::Ordering;
use opendeskphone_state::{
    clock, Command, CommandError, DiscoveredServer, DiscoverySource, Envelope, Event, ProvisionPhase,
    ProvisionUser, Provisioning, Reply, Store, Subsystem,
};
use dpma_proto::cert::PhoneIdentity;
use dpma_proto::message::Incoming;
use dpma_proto::{errors, headers, types};
use md5::{Digest, Md5};
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// How one provisioning attempt ended.
enum Outcome {
    Provisioned,
    /// Waiting for `StartProvisioning { server }`.
    NeedServer,
    /// Waiting for `SelectUser`.
    NeedUser,
}

struct Fail {
    /// What went wrong, in technical terms.
    reason: String,
    code: Option<i32>,
    /// Trying again by itself may help (timeouts, transport).
    retry: bool,
    /// What the user is told ([`Machine::advice`]).
    cause: Cause,
}

/// The kind of a failure, for what the phone's user is told of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    /// The server did not answer, or could not be reached.
    NoAnswer,
    /// The server answered with an error code (`Fail::code`).
    Server,
    /// The server's answer made no sense to the phone.
    Answer,
    /// The configuration the server gave could not be used.
    Config,
    /// The phone's identity could not be made or kept.
    Identity,
}

/// A server's address as a person would write it: without `sip:` and the
/// user before `@`.
fn shown_address(address: &str) -> &str {
    let a = address.trim();
    let a = a.strip_prefix("sips:").or_else(|| a.strip_prefix("sip:")).unwrap_or(a);
    a.rsplit_once('@').map_or(a, |(_, host)| host)
}

/// A `SelectUser` that has not ended yet.
struct Selection {
    /// The user and the token the phone had when the command came, while
    /// the PBX has not granted the new user a token: what a refused login
    /// goes back to.
    previous: Option<(Option<String>, Vec<u8>)>,
}

impl From<DpmaError> for Fail {
    fn from(e: DpmaError) -> Fail {
        match e {
            DpmaError::Server { code, text } => {
                Fail { reason: text, code: Some(code), retry: code == errors::msg::SESSION_TIMEOUT, cause: Cause::Server }
            }
            e @ (DpmaError::Transport(_) | DpmaError::Timeout) => Fail { reason: e.to_string(), code: None, retry: true, cause: Cause::NoAnswer },
            other => Fail { reason: other.to_string(), code: None, retry: true, cause: Cause::Answer },
        }
    }
}

pub(crate) struct Machine {
    store: Store,
    cfg: DpmaConfig,
    link: Arc<Link>,
    files: ClientFiles,
    sink: Arc<dyn ConfigSink>,
    net_up: bool,
    option66: Option<String>,
    mdns: Vec<DiscoveredServer>,
    manual: Option<String>,
    user: Option<String>,
    pin: Option<String>,
    standalone: bool,
    attached: bool,
    server: Option<DiscoveredServer>,
    firmware: Arc<FirmwareCtl>,
    /// What the server's last HandshakeResponse required for a token and
    /// for the user list (lowercase).  They hold for the session, so a
    /// pass that reuses the session (after `digium.config.update`) still
    /// knows them.
    token_reqs: String,
    userlist_reqs: String,
    /// A server (by SIP URI) that answered the single-certificate
    /// Handshake after leaving the chain form unanswered.
    legacy_only: Option<String>,
    /// The pass that fetches the files the configuration names.
    resource_task: Option<tokio::task::JoinHandle<()>>,
    /// The `SelectUser` under way: until it ends, nothing saved for the
    /// user before stands in for what the command named.
    selecting: Option<Selection>,
    /// The user canceled the contact with a server: the phone waits for
    /// a choice of server, the saved one too, until a `StartProvisioning`
    /// or a `Reprovision`.
    choosing: bool,
}

/// The two bodies a Handshake can carry (`dpma-proto`'s `cert`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandshakeForm {
    /// Three certificates: one for servers that read only the first, the
    /// provisioning CA's, and the phone's.  The server answers with
    /// `Handshake-Version: 1` and encrypts to the last certificate.
    Chain,
    /// The phone's certificate alone, as stock firmware sends it (seen in
    /// the D60's 2_9_28: the body is its `phonecert.pem`).
    Legacy,
}

fn sip_uri(address: &str) -> String {
    let a = address.trim();
    if a.starts_with("sip:") || a.starts_with("sips:") { a.to_string() } else { format!("sip:proxy@{a}") }
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn header_text(m: &Incoming, name: &str) -> String {
    m.first.header(name).map(|v| String::from_utf8_lossy(v).trim().to_string()).unwrap_or_default()
}

impl Machine {
    pub fn new(
        store: Store,
        cfg: DpmaConfig,
        link: Arc<Link>,
        files: ClientFiles,
        sink: Arc<dyn ConfigSink>,
        firmware: Arc<FirmwareCtl>,
    ) -> Machine {
        Machine {
            store,
            cfg,
            link,
            files,
            sink,
            net_up: false,
            option66: None,
            mdns: Vec::new(),
            manual: None,
            user: None,
            pin: None,
            standalone: false,
            attached: false,
            server: None,
            firmware,
            token_reqs: String::new(),
            userlist_reqs: String::new(),
            legacy_only: None,
            resource_task: None,
            selecting: None,
            choosing: false,
        }
    }

    /// What the phone's user is told of a failure: what happened, in a
    /// sentence, and what to do about it.  (`Fail::reason` says it for an
    /// administrator.)
    fn advice(&self, f: &Fail) -> String {
        use errors::msg;
        let server = match self.server.as_ref().map(|s| shown_address(&s.address)) {
            Some(a) if !a.is_empty() => format!("The server at \u{201c}{a}\u{201d}"),
            _ => "The server".to_string(),
        };
        match (f.cause, f.code) {
            (Cause::NoAnswer, _) => format!("{server} did not answer. Check the address and the network cable."),
            (Cause::Server, Some(msg::INVALID_PASSWORD)) => "The PIN was not accepted. Try again with the extension's PIN.".into(),
            (Cause::Server, Some(msg::USER_NOT_FOUND)) => "The server does not know that extension. Try again, or ask your administrator.".into(),
            (Cause::Server, Some(msg::INVALID_CERTIFICATE)) => format!("{server} did not accept this phone. Ask your administrator to allow it."),
            (Cause::Server, Some(msg::SESSION_TIMEOUT | msg::INVALID_TOKEN)) => format!("{server} ended the phone's session. Try again."),
            (Cause::Server, Some(msg::INVALID_CONFIG_PASSWORD | msg::CREDENTIALS_NOT_PROVIDED)) => {
                format!("{server} asked for a password this phone does not have. Ask your administrator.")
            }
            (Cause::Server, _) => format!("{server} turned the phone away. Try again, or ask your administrator."),
            (Cause::Answer, _) => format!("{server} answered in a way this phone does not understand. Ask your administrator."),
            (Cause::Config, _) => "The phone could not use the settings from the server. Ask your administrator.".into(),
            (Cause::Identity, _) => "The phone could not prepare its security certificate. Restart the phone and try again.".into(),
        }
    }

    fn phase(&self, phase: ProvisionPhase) {
        self.store.update::<Provisioning, _>(|p| p.phase = phase);
    }

    pub async fn run(
        mut self,
        mut input: mpsc::UnboundedReceiver<Input>,
        mut events: mpsc::UnboundedReceiver<Incoming>,
        commands: opendeskphone_state::CommandReceiver,
    ) {
        let mut want = false;
        let mut full = true;
        let mut retry_at: Option<tokio::time::Instant> = None;
        let mut beat = tokio::time::interval(Duration::from_secs(5));
        let mut ping_at = tokio::time::Instant::now() + Duration::from_secs(self.cfg.ping_secs.max(1));
        loop {
            let retry = async {
                match retry_at {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            };
            let ping = async {
                if self.attached && self.cfg.ping_secs > 0 {
                    tokio::time::sleep_until(ping_at).await
                } else {
                    std::future::pending().await
                }
            };
            tokio::select! {
                i = input.recv() => match i {
                    Some(Input::Network { up, option66, mdns }) => {
                        let came_up = up && !self.net_up;
                        // Discovery inputs are reported again whenever the
                        // integrator looks; only a change is a reason to
                        // start over.
                        let changed = up != self.net_up || option66 != self.option66 || mdns != self.mdns;
                        self.net_up = up;
                        self.option66 = option66;
                        self.mdns = mdns;
                        if !up {
                            self.attached = false;
                            self.link.forget();
                            self.stop_resources();
                            self.store.update::<Provisioning, _>(|p| p.attached = false);
                        } else if (came_up || (changed && !self.attached)) && !self.standalone {
                            want = true;
                            full = true;
                        } else if changed {
                            // Attached: the server in use stays, and the
                            // list shown to the user follows.
                            let (_, found) = self.pick_server();
                            self.store.update::<Provisioning, _>(|p| p.discovered = found);
                        }
                    }
                    None => break,
                },
                env = commands.next() => match env {
                    Some(env) => {
                        if self.command(env) {
                            want = true;
                            full = true;
                        }
                    }
                    None => break,
                },
                ev = events.recv() => match ev {
                    Some(ev) => {
                        if let Some(f) = self.event(ev) {
                            want = true;
                            full = f;
                        }
                    }
                    None => break,
                },
                _ = retry => {
                    retry_at = None;
                    want = true;
                }
                _ = ping => {
                    ping_at = tokio::time::Instant::now() + Duration::from_secs(self.cfg.ping_secs.max(1));
                    match self.link.request(types::PING_REQUEST, Vec::new(), |_| {}).await {
                        Ok(r) if r.error == 0 => {}
                        other => {
                            log::warn!("dpma: ping failed ({}); starting over", match other {
                                Ok(r) => format!("error {}", r.error),
                                Err(e) => e.to_string(),
                            });
                            self.attached = false;
                            self.store.update::<Provisioning, _>(|p| p.attached = false);
                            want = true;
                            full = true;
                        }
                    }
                }
                _ = beat.tick() => self.store.heartbeat(Subsystem::Dpma),
            }
            while want && self.net_up && !self.standalone {
                want = false;
                retry_at = None;
                // The pass.  A server that does not answer takes most of a
                // minute to give up on, and the user may cancel that: a
                // `CancelProvisioning` ends the pass where it is.  Other
                // commands wait for the end of the pass, as they always
                // have.
                let mut held = Vec::new();
                let result = {
                    let pass = self.provision(full);
                    tokio::pin!(pass);
                    let mut open = true;
                    loop {
                        tokio::select! {
                            r = &mut pass => break Some(r),
                            env = commands.next(), if open => match env {
                                Some(env) => {
                                    let cancel = matches!(env.command, Command::CancelProvisioning);
                                    held.push(env);
                                    if cancel {
                                        break None;
                                    }
                                }
                                None => open = false,
                            },
                        }
                    }
                };
                full = true;
                let Some(result) = result else {
                    // Canceled: what the pass began is let go, a user
                    // being chosen goes back to the one before, and the
                    // command has the user choose a server.
                    log::info!("dpma: provisioning canceled");
                    self.link.forget();
                    self.attached = false;
                    if let Some(selection) = self.selecting.take() {
                        let canceled = Fail { reason: "canceled".into(), code: None, retry: false, cause: Cause::NoAnswer };
                        self.selected(selection, &Err(canceled));
                    }
                    for env in held {
                        want |= self.command(env);
                    }
                    continue;
                };
                match &result {
                    Ok(Outcome::Provisioned) => {
                        self.attached = true;
                        ping_at = tokio::time::Instant::now() + Duration::from_secs(self.cfg.ping_secs.max(1));
                    }
                    Ok(Outcome::NeedServer) => self.phase(ProvisionPhase::AwaitingServer),
                    Ok(Outcome::NeedUser) => self.phase(ProvisionPhase::AwaitingUser),
                    Err(f) => {
                        log::warn!("dpma: provisioning failed: {} (code {:?})", f.reason, f.code);
                        self.attached = false;
                        let reason = self.advice(f);
                        self.store.update::<Provisioning, _>(|p| {
                            p.attached = false;
                            p.phase = ProvisionPhase::Failed { reason, code: f.code, detail: f.reason.clone() };
                        });
                        if f.retry {
                            retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(self.cfg.retry_secs));
                        }
                    }
                }
                if let Some(selection) = self.selecting.take()
                    && self.selected(selection, &result)
                {
                    // The assignment the phone had comes back.
                    retry_at = None;
                    want = true;
                }
                for env in held {
                    want |= self.command(env);
                }
            }
        }
    }

    /// Handles a command; returns whether to (re)start provisioning.
    fn command(&mut self, env: Envelope) -> bool {
        let (command, responder) = env.split();
        let mut start = false;
        let result = match command {
            Command::StartProvisioning { server } => {
                if let Some(s) = server.filter(|s| !s.trim().is_empty()) {
                    self.manual = Some(s);
                }
                self.standalone = false;
                self.choosing = false;
                start = true;
                Ok(Reply::Done)
            }
            Command::CancelProvisioning => {
                // (A pass under way was ended before this came here.)
                // The server named in this session is let go, and the
                // phone waits for the user's choice.
                self.manual = None;
                self.choosing = true;
                self.attached = false;
                self.link.forget();
                let (_, found) = self.pick_server();
                self.store.update::<Provisioning, _>(|p| {
                    p.attached = false;
                    p.discovered = found;
                    p.phase = ProvisionPhase::AwaitingServer;
                });
                Ok(Reply::Done)
            }
            Command::SelectUser { id, pin } => {
                // The PBX decides about the user: the token the phone
                // holds is for the user before, and with it the PBX would
                // never be asked.  It is kept for a refusal.
                let previous = match self.selecting.take() {
                    Some(earlier) => earlier.previous,
                    None => self.link.token().map(|token| (self.user.clone(), token)),
                };
                self.link.set_token(None);
                self.selecting = Some(Selection { previous });
                self.user = Some(id).filter(|u| !u.is_empty());
                self.pin = Some(pin).filter(|p| !p.is_empty());
                self.standalone = false;
                self.store.update::<Provisioning, _>(|p| p.selection = None);
                start = true;
                Ok(Reply::Done)
            }
            Command::Reprovision => {
                self.standalone = false;
                self.choosing = false;
                start = true;
                Ok(Reply::Done)
            }
            Command::ForgetProvisioning => {
                self.link.forget();
                self.link.set_token(None);
                self.manual = None;
                self.user = None;
                self.pin = None;
                self.attached = false;
                self.server = None;
                self.token_reqs.clear();
                self.userlist_reqs.clear();
                self.legacy_only = None;
                self.selecting = None;
                self.choosing = false;
                self.stop_resources();
                let _ = std::fs::remove_file(&self.files.identity_cert);
                let _ = std::fs::remove_file(&self.files.identity_key);
                let r = self.sink.forget();
                self.store.set(Provisioning::default());
                self.standalone = false;
                start = true;
                r.map(|_| Reply::Done).map_err(CommandError::Failed)
            }
            Command::SkipProvisioning => {
                self.standalone = true;
                self.attached = false;
                self.link.forget();
                self.stop_resources();
                self.store.update::<Provisioning, _>(|p| {
                    p.attached = false;
                    p.phase = ProvisionPhase::Standalone;
                });
                Ok(Reply::Done)
            }
            other => Err(CommandError::Invalid(format!("not a provisioning command: {other:?}"))),
        };
        responder.reply(result);
        start
    }

    /// Handles a server-initiated message.  `Some(full)` asks for another
    /// provisioning pass, with or without a new handshake.
    fn event(&mut self, ev: Incoming) -> Option<bool> {
        let name = header_text(&ev, headers::EVENT_NAME);
        if ev.error == errors::msg::SESSION_TIMEOUT {
            // Reassignment (`digium.assignment.update` arrives as ID
            // `bootcReset` with -398) or an expired session: like stock,
            // go back to the handshake.  A reassignment also invalidates
            // the token and the chosen user.
            if ev.id == b"bootcReset" {
                self.link.set_token(None);
                self.user = None;
                self.pin = None;
                self.save_configurator(|c| {
                    c.remove("config_server_token");
                    c.remove("config_server_user");
                });
            }
            self.attached = false;
            self.link.forget();
            self.store.update::<Provisioning, _>(|p| p.attached = false);
            return Some(true);
        }
        if name == types::events::CONFIG_UPDATE {
            if xml::deletes_token(&ev.payload) {
                self.link.set_token(None);
                self.save_configurator(|c| c.remove("config_server_token"));
            }
            self.store.emit(Event::Push { name, body: String::from_utf8_lossy(&ev.payload).into_owned() });
            return Some(false);
        }
        if !name.is_empty() {
            self.store.emit(Event::Push { name, body: String::from_utf8_lossy(&ev.payload).into_owned() });
        } else {
            log::debug!("dpma: unexpected message id {:?}", String::from_utf8_lossy(&ev.id));
        }
        None
    }

    /// Ends a `SelectUser` with the pass that carried it out: tells the
    /// state how it ended, and, when the PBX did not take the user, gives
    /// the phone back the user and the token it had.  Returns whether
    /// another pass is wanted for that.
    fn selected(&mut self, selection: Selection, result: &Result<Outcome, Fail>) -> bool {
        let outcome = match result {
            Ok(Outcome::Provisioned) => Ok(()),
            // Logged out, and the PBX has no configuration for a phone
            // without a user: the phone waits for one.
            Ok(Outcome::NeedUser) if self.user.is_none() => {
                self.save_configurator(|c| {
                    c.remove("config_server_token");
                    c.remove("config_server_user");
                    c.remove("config_user_password");
                });
                self.store.update::<Provisioning, _>(|p| p.assigned_user.clear());
                Ok(())
            }
            Ok(Outcome::NeedUser) => Err("the PBX needs a PIN for the user".to_string()),
            Ok(Outcome::NeedServer) => Err("the phone has no PBX".to_string()),
            Err(f) => Err(match f.code {
                Some(code) => format!("{} ({code})", f.reason),
                None => f.reason.clone(),
            }),
        };
        let restore = match (&outcome, selection.previous) {
            (Err(_), Some((user, token))) => {
                self.user = user;
                self.pin = None;
                self.link.set_token(Some(&token));
                true
            }
            _ => false,
        };
        self.store.update::<Provisioning, _>(|p| p.selection = Some(outcome));
        restore
    }

    fn configurator(&self) -> Configurator {
        self.sink.configurator().map(|x| Configurator::parse(&x)).unwrap_or_default()
    }

    fn save_configurator(&self, f: impl FnOnce(&mut Configurator)) {
        let mut c = self.configurator();
        f(&mut c);
        if let Err(e) = self.sink.write_configurator(&c.render()) {
            log::warn!("dpma: cannot save configurator.xml: {e}");
        }
    }

    /// The server to use, and what discovery offers: DHCP option 66 in its
    /// `sip:` form, then the mDNS answers.
    ///
    /// Only a server the user named in this session (picked from the list
    /// or typed) or the saved one (`config_server_url`, written once a
    /// server has provisioned the phone) is used without asking.  A phone
    /// with neither, as after a factory reset, lists what discovery found
    /// and waits for a `StartProvisioning`.  The server menu sends it: for
    /// the user's choice, or by itself for a lone entry after stock's
    /// six-second countdown (`choosePbx.js`, `startSingleCountdown`).  So
    /// does a phone whose user canceled the contact with its server, with
    /// the saved server listed first.
    fn pick_server(&self) -> (Option<DiscoveredServer>, Vec<DiscoveredServer>) {
        let mut found = Vec::new();
        if let Some(o) = self.option66.as_deref().map(str::trim).filter(|o| o.starts_with("sip:") || o.starts_with("sips:")) {
            found.push(DiscoveredServer { source: DiscoverySource::DhcpOption66, name: o.to_string(), address: o.to_string() });
        }
        found.extend(self.mdns.iter().cloned());
        if let Some(m) = &self.manual {
            // A pick from the list stays what discovery said it was.
            let picked = found.iter().find(|f| f.address == *m).cloned();
            let server = picked.unwrap_or_else(|| DiscoveredServer { source: DiscoverySource::Manual, name: m.clone(), address: m.clone() });
            return (Some(server), found);
        }
        if let Some(url) = self.configurator().saved_server() {
            let saved = DiscoveredServer { source: DiscoverySource::Saved, name: url.to_string(), address: url.to_string() };
            // After a cancel, the saved server is one of the choices.
            if self.choosing {
                if !found.iter().any(|f| f.address == saved.address) {
                    found.insert(0, saved);
                }
                return (None, found);
            }
            return (Some(saved), found);
        }
        (None, found)
    }

    /// The phone identity: minted on first use with the provisioning CA built
    /// into `dpma-proto`, and kept under `/nvdata/opendeskphone/`.  Stock's
    /// own `phonekey.pem`/`phonecert.pem` are never touched.
    fn identity(&self) -> Result<PhoneIdentity, Fail> {
        let fail = |reason: String| Fail { reason, code: None, retry: false, cause: Cause::Identity };
        if let (Ok(cert), Ok(key)) = (std::fs::read(&self.files.identity_cert), std::fs::read(&self.files.identity_key)) {
            if let Ok(private_key) = rsa::RsaPrivateKey::from_pkcs1_pem(&String::from_utf8_lossy(&key)) {
                return Ok(PhoneIdentity { certificate_pem: cert, private_key });
            }
            log::warn!("dpma: the saved identity key is unreadable; minting a new identity");
        }
        let id = PhoneIdentity::generate().map_err(|e| fail(format!("cannot mint an identity: {}", e.message())))?;
        let pem = id
            .private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .map_err(|e| fail(format!("cannot encode the identity key: {e}")))?;
        let written = |e: std::io::Error| fail(format!("cannot keep the identity: {e}"));
        write_atomic(&self.files.identity_cert, &id.certificate_pem).map_err(written)?;
        write_atomic_private(&self.files.identity_key, pem.as_bytes()).map_err(written)?;
        Ok(id)
    }

    /// The Handshake: the certificate-chain form first, and the phone's
    /// certificate alone when a server leaves that unanswered (a server
    /// that refuses the certificates sends nothing at all).
    ///
    /// The phone has one identity, and it stands in both places of the
    /// chain form that carry a phone certificate.  A server that reads the
    /// whole chain encrypts the common password to the last certificate,
    /// and one that reads only the first to the first: the same key either
    /// way.
    async fn handshake(&mut self, identity: &PhoneIdentity, server_uri: &str) -> Result<Incoming, Fail> {
        let forms: &[HandshakeForm] = if self.legacy_only.as_deref() == Some(server_uri) {
            &[HandshakeForm::Legacy]
        } else {
            &[HandshakeForm::Chain, HandshakeForm::Legacy]
        };
        for (n, form) in forms.iter().enumerate() {
            let body = match form {
                HandshakeForm::Chain => identity.chain_body(identity),
                HandshakeForm::Legacy => identity.legacy_body(),
            };
            self.link.forget();
            match self.link.handshake(body).await {
                Ok(resp) => {
                    if resp.error == 0 {
                        log::info!(
                            "dpma: handshake answered ({}{})",
                            match form {
                                HandshakeForm::Chain => "certificate chain",
                                HandshakeForm::Legacy => "single certificate",
                            },
                            if resp.handshake_version.is_empty() {
                                String::new()
                            } else {
                                format!(", Handshake-Version {}", String::from_utf8_lossy(&resp.handshake_version))
                            }
                        );
                        if *form == HandshakeForm::Legacy && n > 0 {
                            self.legacy_only = Some(server_uri.to_string());
                        }
                    }
                    return Ok(resp);
                }
                Err(DpmaError::Timeout) if n + 1 < forms.len() => {
                    log::info!("dpma: no answer to the certificate-chain Handshake; sending the single certificate");
                }
                // A refused certificate gets no HandshakeResponse at all.
                Err(DpmaError::Timeout) => {
                    return Err(Fail {
                        reason: "no HandshakeResponse (server unreachable, or it refused the phone certificate)".into(),
                        code: None,
                        retry: true,
                        cause: Cause::NoAnswer,
                    });
                }
                Err(other) => return Err(other.into()),
            }
        }
        unreachable!("the last form returns")
    }

    async fn provision(&mut self, full: bool) -> Result<Outcome, Fail> {
        // The files of the previous configuration are not fetched while a
        // new one may be on its way.
        self.stop_resources();
        if full || !self.link.has_session() {
            let (server, found) = self.pick_server();
            // A phone that waits for its user stays in that phase while
            // the list follows discovery: the screen that shows the list
            // (and counts down to a lone entry) is not started over.
            let Some(server) = server else {
                self.store.update::<Provisioning, _>(|p| {
                    p.discovered = found;
                    p.phase = ProvisionPhase::AwaitingServer;
                });
                return Ok(Outcome::NeedServer);
            };
            self.store.update::<Provisioning, _>(|p| p.discovered = found);
            let server_uri = sip_uri(&server.address);
            self.link.set_server(&server_uri);
            self.server = Some(server.clone());
            self.store.update::<Provisioning, _>(|p| {
                p.server = Some(server);
                p.phase = ProvisionPhase::Handshaking;
                p.attached = false;
            });
            let identity = self.identity()?;
            self.token_reqs.clear();
            self.userlist_reqs.clear();
            let resp = self.handshake(&identity, &server_uri).await?;
            if resp.error != 0 {
                return Err(Fail { reason: errors::msg_error_text(resp.error).into(), code: Some(resp.error), retry: true, cause: Cause::Server });
            }
            self.link.accept_handshake(&resp, &identity.private_key)?;
            self.token_reqs = header_text(&resp, headers::TOKEN_AUTH_REQUIREMENTS).to_ascii_lowercase();
            self.userlist_reqs = header_text(&resp, headers::USERLIST_AUTH_REQUIREMENTS).to_ascii_lowercase();
            let ua = {
                let v = header_text(&resp, headers::USER_AGENT);
                if v.is_empty() { header_text(&resp, "Server") } else { v }
            };
            self.store.update::<Provisioning, _>(|p| {
                p.server_user_agent = ua;
                p.attached = true;
            });
        }
        let has = |reqs: &str, what: &str| reqs.split(',').any(|r| r.trim() == what);
        let (token_reqs, userlist_reqs) = (self.token_reqs.clone(), self.userlist_reqs.clone());

        // Saved state applies only to the server it was saved for.
        let saved = self.configurator();
        let server_addr = self.server.as_ref().map(|s| s.address.clone()).unwrap_or_default();
        let same_server = saved.get("config_server_url").is_none_or(|u| sip_uri(u) == sip_uri(&server_addr));
        // What is saved for the user is the user before's while a
        // `SelectUser` is under way.
        let fresh = self.selecting.is_some();
        if self.user.is_none() && same_server && !fresh {
            self.user = saved.get("config_server_user").map(str::to_string);
        }
        let saved_user_pass = saved.get("config_user_password").filter(|_| same_server && !fresh).map(str::to_string);
        let saved_cfg_pass = saved.get("config_server_password").filter(|_| same_server).map(str::to_string);
        if let (true, true, Some(t)) = (self.link.token().is_none(), same_server && !fresh, saved.get("config_server_token")) {
            self.link.set_token(Some(t.as_bytes()));
        }
        let user = self.user.clone().unwrap_or_default();

        // A token from an earlier boot: verify it.  Only a digest equal to
        // the cached one lets us skip the download ("md5sum_match").
        if full && self.link.token().is_some() {
            let u = user.clone();
            let resp = self.link.request(types::VERIFY_REQUEST, Vec::new(), |r| r.username = u.into_bytes()).await?;
            if resp.error == 0 {
                let md5 = header_text(&resp, headers::MD5);
                if !md5.is_empty() && self.sink.cached_md5().as_deref() == Some(md5.as_str()) {
                    // Same configuration as last boot; the firmware it
                    // provisions is still checked, as stock does on
                    // every boot.
                    if let Some(saved) = self.sink.fetched_config() {
                        self.check_firmware(saved);
                    }
                    self.done(&user, &md5);
                    self.start_resources();
                    return Ok(Outcome::Provisioned);
                }
            } else if resp.error == errors::msg::SESSION_TIMEOUT {
                return Err(Fail { reason: "session timeout".into(), code: Some(resp.error), retry: true, cause: Cause::Server });
            } else {
                log::info!("dpma: the saved token was refused ({}); asking for a new one", resp.error);
                self.link.set_token(None);
            }
        }

        if self.link.token().is_none() {
            let need_user = has(&token_reqs, "user");
            let need_pass = has(&token_reqs, "pass");
            let need_cfg = has(&token_reqs, "configpass");
            let pass = self.pin.clone().or(saved_user_pass.clone());
            let cfg_pass = self.pin.clone().or(saved_cfg_pass.clone());
            let missing = (need_user && user.is_empty()) || (need_pass && pass.is_none()) || (need_cfg && cfg_pass.is_none());
            if missing {
                // Offer the user list when the server gives one out.
                let mut users = Vec::new();
                let can_list = match userlist_reqs.as_str() {
                    "disabled" => false,
                    "configpass" => cfg_pass.is_some(),
                    _ => true,
                };
                if can_list {
                    let cp = cfg_pass.clone();
                    match self
                        .link
                        .request(types::USERLIST_REQUEST, Vec::new(), |r| {
                            if let Some(p) = cp {
                                r.config_password = p.into_bytes();
                            }
                        })
                        .await
                    {
                        Ok(r) if r.error == 0 => users = parse_users(&body_of(&r), need_pass || need_cfg),
                        Ok(r) => log::info!("dpma: no user list: error {}", r.error),
                        Err(e) => log::info!("dpma: no user list: {e}"),
                    }
                }
                self.store.update::<Provisioning, _>(|p| p.users = users);
                return Ok(Outcome::NeedUser);
            }
            let (u, p, c) = (user.clone(), pass.clone(), cfg_pass.clone());
            let resp = self
                .link
                .request(types::TOKEN_REQUEST, Vec::new(), |r| {
                    r.username = u.into_bytes();
                    if let (true, Some(p)) = (need_pass, p) {
                        r.password = p.into_bytes();
                    }
                    if let (true, Some(c)) = (need_cfg, c) {
                        r.config_password = c.into_bytes();
                    }
                })
                .await?;
            if resp.error != 0 {
                // Wrong PIN or user: ask again rather than looping.
                self.pin = None;
                return Err(Fail {
                    reason: errors::msg_error_text(resp.error).into(),
                    code: Some(resp.error),
                    retry: resp.error == errors::msg::SESSION_TIMEOUT,
                    cause: Cause::Server,
                });
            }
            if resp.config_token.is_empty() {
                return Err(Fail { reason: "the TokenResponse has no token".into(), code: None, retry: true, cause: Cause::Answer });
            }
            self.link.set_token(Some(&resp.config_token));
            // The PBX took the user: there is no going back to the one
            // before.
            if let Some(selection) = &mut self.selecting {
                selection.previous = None;
            }
            let assigned = String::from_utf8_lossy(&resp.username).into_owned();
            if self.user.is_none() && !assigned.is_empty() {
                self.user = Some(assigned);
            }
            // Keep what was used, in stock's own settings, so both this
            // firmware and stock find it on the next boot.
            let token = String::from_utf8_lossy(&resp.config_token).into_owned();
            let user_now = self.user.clone().unwrap_or_default();
            let source = self.server.as_ref().map(|s| s.source);
            self.save_configurator(|c| {
                c.set("config_server_type", if source == Some(DiscoverySource::DhcpOption66) { "boot66" } else { "digium" });
                c.set("config_server_manual", if source == Some(DiscoverySource::Manual) { "1" } else { "0" });
                c.set("config_server_url", &server_addr);
                c.set("config_server_user", &user_now);
                if need_cfg {
                    c.set("config_server_password", cfg_pass.as_deref().unwrap_or(""));
                }
                if need_pass {
                    c.set("config_user_password", pass.as_deref().unwrap_or(""));
                }
                c.set("config_server_token", &token);
            });
        }

        // Download.
        self.phase(ProvisionPhase::FetchingConfig);
        let user = self.user.clone().unwrap_or_default();
        let u = user.clone();
        let resp = self.link.request(types::CONFIG_REQUEST, Vec::new(), |r| r.username = u.into_bytes()).await?;
        if resp.error != 0 {
            if resp.error == errors::msg::INVALID_TOKEN {
                self.link.set_token(None);
                self.save_configurator(|c| c.remove("config_server_token"));
            }
            return Err(Fail {
                reason: errors::msg_error_text(resp.error).into(),
                code: Some(resp.error),
                retry: matches!(resp.error, errors::msg::INVALID_TOKEN | errors::msg::SESSION_TIMEOUT),
                cause: Cause::Server,
            });
        }
        let config = body_of(&resp);
        if config.is_empty() {
            return Err(Fail { reason: "the ConfigResponse is empty".into(), code: None, retry: true, cause: Cause::Answer });
        }
        // Stock stores the server's digest verbatim when it sends one, and
        // otherwise the MD5 of the downloaded bytes.
        let md5 = {
            let h = header_text(&resp, headers::MD5);
            if h.is_empty() { md5_hex(&config) } else { h }
        };
        let offered = xml::offered_firmware(&config);
        // The setting as it will be with this configuration in effect: a
        // configuration can set it and offer firmware at once.
        if let Some(ignore) = self.sink.ignore_provisioned_firmware(&config) {
            self.firmware.ignore.store(ignore, Ordering::SeqCst);
        }
        let ignore = self.firmware.ignore.load(Ordering::SeqCst);
        if ignore && offered.is_some() {
            log::info!("dpma: ignoring provisioned firmware {} (ignore_provisioned_firmware is set)", offered.as_deref().unwrap_or(""));
        }
        let content = if ignore { xml::strip_firmware(&config) } else { config.clone() };
        self.store.update::<Provisioning, _>(|p| {
            p.offered_firmware = offered;
            p.phase = ProvisionPhase::Applying;
        });
        self.sink.apply(&content, &md5).map_err(|e| Fail { reason: format!("cannot apply the configuration: {e}"), code: None, retry: false, cause: Cause::Config })?;
        // Applying tells the integrator, which reports the setting now in
        // effect; `check_firmware` goes by that.
        if !ignore {
            self.check_firmware(config);
        }
        self.done(&user, &md5);
        self.start_resources();
        Ok(Outcome::Provisioned)
    }

    fn firmware_policy(&self) -> crate::firmware::Policy {
        // `Digium <model> <version>`.
        let mut words = self.cfg.user_agent.split_whitespace().skip(1);
        let (model, running) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let mac: Vec<&str> = (0..self.cfg.mac.len() / 2).map(|i| &self.cfg.mac[i * 2..i * 2 + 2]).collect();
        let mut policy = crate::firmware::Policy::new(model, running, &mac.join(":"));
        policy.mac_in_user_agent = self.cfg.mac_in_user_agent;
        policy
    }

    fn stop_resources(&mut self) {
        if let Some(task) = self.resource_task.take() {
            task.abort();
        }
    }

    /// Fetches the files the configuration in effect names, off this task:
    /// the session stays free for pushes and app requests meanwhile.
    fn start_resources(&mut self) {
        self.stop_resources();
        let wanted = self.sink.wanted_resources();
        // Stock JavaScript apps the server pushes: skipped, and said so.
        resources::log_skipped_apps(&wanted.apps);
        let Some(files) = self.sink.files() else { return };
        let sink = self.sink.clone();
        let fetch = Fetcher {
            link: self.link.clone(),
            http: match self.firmware.http.lock().unwrap().clone() {
                Some(source) => source,
                None => Arc::new(crate::fetch::HttpSource::new()),
            },
            user_agent: self.firmware_policy().user_agent(),
        };
        self.resource_task = Some(tokio::spawn(async move {
            let report = resources::sync(&wanted, &files, &fetch).await;
            if report.fetched + report.matched + report.failed + report.removed > 0 {
                log::info!(
                    "dpma: provisioned files: {} downloaded, {} unchanged by digest, {} failed, {} removed",
                    report.fetched,
                    report.matched,
                    report.failed,
                    report.removed
                );
            }
            if report.changed {
                sink.resources_changed();
            }
        }));
    }

    /// Compares the firmware a fetched configuration provisions with the
    /// running image and installs it when they differ.  Runs on its own
    /// thread: the download and the flash write take minutes.
    fn check_firmware(&self, config: Vec<u8>) {
        if self.firmware.ignore.load(Ordering::SeqCst) {
            return;
        }
        let Some((source, installer)) = self.firmware.parts.lock().unwrap().clone() else {
            log::debug!("dpma: no firmware updater; provisioned firmware is not acted on");
            return;
        };
        let runner = crate::firmware::Runner {
            store: self.store.clone(),
            policy: self.firmware_policy(),
            attempt_file: self.files.firmware_attempt.clone(),
            source,
            installer,
            running: self.firmware.running.clone(),
        };
        let spawned = std::thread::Builder::new().name("firmware".into()).spawn(move || {
            runner.run(&config);
        });
        if let Err(e) = spawned {
            log::error!("dpma: cannot start the firmware check: {e}");
        }
    }

    fn done(&self, user: &str, md5: &str) {
        self.store.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::Provisioned;
            p.attached = true;
            p.assigned_user = user.to_string();
            p.config_md5 = md5.to_string();
            p.last_success_unix = Some(clock::unix_secs());
            p.users.clear();
        });
    }
}

/// Resource downloads: FileRequests on the session, and HTTP through the
/// blocking client on a thread of its own.
struct Fetcher {
    link: Arc<Link>,
    http: Arc<dyn crate::firmware::FirmwareSource>,
    user_agent: String,
}

#[async_trait::async_trait]
impl resources::Fetch for Fetcher {
    async fn file(&self, name: &str) -> Result<Vec<u8>, DpmaError> {
        self.link.file(name).await
    }

    async fn http(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let (http, url, user_agent) = (self.http.clone(), url.to_string(), self.user_agent.clone());
        tokio::task::spawn_blocking(move || {
            let body = http.open(&url, &user_agent).map_err(|e| match e.status {
                Some(status) => format!("HTTP {status}: {}", e.message),
                None => e.message,
            })?;
            let mut data = Vec::new();
            body.take(max as u64 + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
            if data.len() > max {
                return Err(format!("more than the {max} bytes allowed"));
            }
            Ok(data)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// `{"users":[{"extension":"100","name":"Desk","assigned":"0"}]}`;
/// `assigned` is a string.
fn parse_users(body: &[u8], needs_pin: bool) -> Vec<ProvisionUser> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Vec::new() };
    let text = |x: &serde_json::Value| match x {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    v.get("users")
        .and_then(|u| u.as_array())
        .map(|list| {
            list.iter()
                .map(|u| {
                    let id = u.get("extension").map(text).unwrap_or_default();
                    let name = u.get("name").map(text).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                    ProvisionUser { id, name, needs_pin }
                })
                .filter(|u| !u.id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_json() {
        let u = parse_users(br#"{"users":[{"extension":"100","name":"Desk","assigned":"0"},{"extension":101,"name":""}]}"#, true);
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].id.as_str(), u[0].name.as_str(), u[0].needs_pin), ("100", "Desk", true));
        assert_eq!((u[1].id.as_str(), u[1].name.as_str()), ("101", "101"));
        assert!(parse_users(b"not json", false).is_empty());
    }

    #[test]
    fn server_uris() {
        assert_eq!(sip_uri("10.0.0.1:5060"), "sip:proxy@10.0.0.1:5060");
        assert_eq!(sip_uri("sip:proxy@pbx;transport=tcp"), "sip:proxy@pbx;transport=tcp");
    }
}
