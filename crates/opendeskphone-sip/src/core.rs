//! The user-agent core: one task that owns call-control policy and the
//! store.  Dialogs, registration, and subscriptions are `rsipstack`'s
//! dialog layer; helper tasks await its futures and post the outcomes back
//! as [`Ev`]s, so nothing here needs a lock.

use crate::config::{AccountConfig, SipConfig};
use crate::media::{MediaControl, MediaEvent};
use crate::sdp::{Crypto, Negotiated, Sdp};
use crate::util::{self, NameAddr, SipUri};
use crate::{IncomingMessage, OutMessage, Resolver};
use opendeskphone_state::{
    AccountId, Blink, CallId, Envelope, Keys, LampColor, LampState, LineKey, LineKeyBinding, RapidDialKey,
    Store, Subsystem, Transport,
};
use rsipstack::dialog::authenticate::{handle_client_authenticate, Credential};
use rsipstack::dialog::dialog::{DialogState, DialogStateSender, TransactionHandle};
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::dialog::invite_dialog::InviteDialog;
use rsipstack::dialog::registration::Registration;
use rsipstack::dialog::subscription::ClientSubscriptionDialog;
use rsipstack::sip::prelude::HeadersExt;
use rsipstack::sip::{Header, Request, Response, StatusCode, Uri};
use rsipstack::transaction::endpoint::EndpointInnerRef;
use rsipstack::transaction::key::{TransactionKey, TransactionRole};
use rsipstack::transaction::transaction::{Transaction, TransactionEventSender};
use rsipstack::transport::SipAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The outcome of one REGISTER exchange.
pub(crate) struct RegOutcome {
    /// Final status, or 0 when there was no answer.
    pub code: u16,
    pub reason: String,
    pub granted: u32,
    pub min_expires: Option<u32>,
    /// Our address as the registrar saw it (Via `received`/`rport`).
    pub public: Option<rsipstack::sip::HostWithPort>,
}

/// Everything that can happen to the core.
pub(crate) enum Ev {
    Cmd(Envelope),
    Incoming(Transaction),
    Dialog(DialogState),
    InviteDone { call: CallId, result: Result<Option<Response>, String> },
    ReinviteDone { call: CallId, hold: bool, result: Result<Option<Response>, String> },
    ReferDone { call: CallId, result: Result<Option<Response>, String> },
    /// The answer to a session refresh sent as UPDATE.
    RefreshDone { call: CallId, generation: u64, result: Result<Option<Response>, String> },
    /// The SDP answer in the ACK of an offerless (re-)INVITE.
    AckBody { call: CallId, body: Vec<u8> },
    Registered { acct: AccountId, generation: u64, outcome: RegOutcome },
    Subscribed { sub: u32, generation: u64, result: Result<Option<Response>, String> },
    Timer(Timer),
    Media(MediaEvent),
    Reconfigure(Box<SipConfig>),
    SendMessage { msg: OutMessage, reply: oneshot::Sender<Result<u16, String>> },
    Subscribe(mpsc::UnboundedSender<IncomingMessage>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
pub(crate) enum Timer {
    Register { acct: AccountId, generation: u64 },
    Keepalive { acct: AccountId, generation: u64 },
    Subscribe { sub: u32, generation: u64 },
    NoAnswer { call: CallId },
    Linger { call: CallId },
    /// Back to the first rapid-dial page, unless the page changed since.
    PageReturn { generation: u64 },
    /// Retry a hold change that could not be sent yet.
    HoldRetry { call: CallId },
    /// Time for us to refresh a session (RFC 4028).
    SessionRefresh { call: CallId, generation: u64 },
    /// The other side did not refresh the session in time.
    SessionExpire { call: CallId, generation: u64 },
    Heartbeat,
}

/// Registration state of one account.  The REGISTER exchanges themselves
/// are `rsipstack`'s [`Registration`]; the failover between the primary
/// and alternate host is ours.
pub(crate) struct Acct {
    pub cfg: AccountConfig,
    /// Using the alternate host.
    pub alt: bool,
    pub generation: u64,
    pub fails: u32,
    pub registered: bool,
    pub reg: Arc<tokio::sync::Mutex<Registration>>,
    /// The address the registrar sees us at, for NAT-aware Contacts.
    pub public: Option<rsipstack::sip::HostWithPort>,
    /// The host of the last successful registration (`true`: the
    /// alternate), to notice a change of server.
    pub served_by_alt: Option<bool>,
}

/// A REFER we received and are acting on: progress NOTIFYs go back on the
/// referring call's dialog.
#[derive(Clone, Debug)]
pub(crate) struct ReferFrom {
    pub call: CallId,
}

pub(crate) struct CallRt {
    pub id: CallId,
    pub acct: AccountId,
    pub alt: bool,
    pub dialog: InviteDialog,
    /// Call-ID and our tag: what identifies the dialog in state events
    /// before and after the remote tag is known.
    pub key: (String, String),
    pub outgoing: bool,
    /// Incoming and unanswered: the transaction's response channel and
    /// request, for the one response the layer cannot build (a redirect).
    pub server: Option<(TransactionEventSender, Request)>,
    pub rtp_port: u16,
    pub session_id: u64,
    pub sdp_version: u64,
    /// Our outstanding offer.
    pub offer: Option<Sdp>,
    /// Their offer, kept until we answer.
    pub remote_offer: Option<Sdp>,
    pub key_material: Option<Crypto>,
    pub neg: Option<Negotiated>,
    pub local_hold: bool,
    pub remote_hold: bool,
    /// A re-INVITE of ours is in flight.
    pub reinvite_busy: bool,
    /// The hold state to move to once the current re-INVITE finishes.
    pub hold_wanted: Option<bool>,
    pub answered: bool,
    pub ended: bool,
    pub alert_info: String,
    pub refer_from: Option<ReferFrom>,
    pub replaces: Option<String>,
    pub referred_by: Option<String>,
    /// Headers an app asked for on the INVITE (kept for a retry).
    pub extra_headers: Vec<(String, String)>,
    /// A 3xx was followed already.
    pub redirected: bool,
    /// The other party's URI (as dialed, or their From).
    pub peer: String,
    /// The session timer in force, if any, and the generation of its
    /// pending timer event.
    pub session: Option<crate::timer::Agreed>,
    pub session_gen: u64,
    /// The interval we ask for (raised by a 422).
    pub session_secs: u32,
    /// What the 2xx to their INVITE says about the session timer.
    pub session_answer: Option<(crate::timer::SessionExpires, bool, crate::timer::Agreed)>,
    /// The peer's `Allow` lists UPDATE (the cheaper refresh).
    pub peer_update: bool,
}

pub(crate) enum SubKind {
    /// `presence` event for a BLF entry: the contact's line state and the
    /// user's status in one document.  As on stock, this is the only
    /// subscription a watched contact has.
    Blf(String),
    Mwi,
}

pub(crate) struct Sub {
    pub id: u32,
    pub kind: SubKind,
    pub acct: AccountId,
    pub uri: String,
    pub call_id: String,
    pub local_tag: String,
    /// Empty until the notifier's tag is known.
    pub remote_tag: String,
    pub dialog: Option<ClientSubscriptionDialog>,
    pub generation: u64,
}

pub(crate) struct Core {
    pub store: Store,
    pub cfg: SipConfig,
    pub media: Arc<dyn MediaControl>,
    pub resolver: Arc<dyn Resolver>,
    pub inner: EndpointInnerRef,
    pub layer: Arc<DialogLayer>,
    pub dialog_tx: DialogStateSender,
    pub tx: mpsc::UnboundedSender<Ev>,
    pub local_port: u16,
    pub accts: Vec<Acct>,
    pub calls: Vec<CallRt>,
    pub subs: Vec<Sub>,
    pub next_sub: u32,
    pub conference: Vec<CallId>,
    pub last_dialed: Option<(String, Option<AccountId>)>,
    pub listeners: Vec<mpsc::UnboundedSender<IncomingMessage>>,
    pub park_extension: String,
    /// Call-IDs of CANCELs that said the call was answered elsewhere.
    pub elsewhere: Arc<std::sync::Mutex<Vec<String>>>,
    /// Registrars (as `TCP host:port`) to which a TCP connection of ours,
    /// marked, is open.
    pub streams: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// The UDP signaling socket, kept to mark it again.
    pub udp: Arc<tokio::net::UdpSocket>,
    /// Stream ports we accept connections on.
    pub tcp_listening: Option<u16>,
    pub tls_listening: Option<u16>,
    /// Counts rapid-dial page changes, for the return-to-first-page timer.
    pub page_changes: u64,
}

pub(crate) fn sip_transport(t: Transport) -> rsipstack::sip::Transport {
    match t {
        Transport::Udp => rsipstack::sip::Transport::Udp,
        Transport::Tcp => rsipstack::sip::Transport::Tcp,
        Transport::Tls => rsipstack::sip::Transport::Tls,
    }
}

pub(crate) fn uri(text: &str) -> Result<Uri, String> {
    Uri::try_from(text).map_err(|e| format!("bad URI {text}: {e}"))
}

/// `;transport=` suffix for URIs we write.
pub(crate) fn transport_param(t: Transport) -> &'static str {
    match t {
        Transport::Udp => "",
        Transport::Tcp => ";transport=tcp",
        Transport::Tls => ";transport=tls",
    }
}

impl Core {
    /// Posts `t` after `d`.
    pub fn after(&self, d: Duration, t: Timer) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(d).await;
            let _ = tx.send(Ev::Timer(t));
        });
    }

    pub fn acct(&self, id: AccountId) -> Option<&Acct> {
        self.accts.iter().find(|a| a.cfg.id == id)
    }
    pub fn acct_mut(&mut self, id: AccountId) -> Option<&mut Acct> {
        self.accts.iter_mut().find(|a| a.cfg.id == id)
    }
    pub fn call(&self, id: CallId) -> Option<&CallRt> {
        self.calls.iter().find(|c| c.id == id)
    }
    pub fn call_mut(&mut self, id: CallId) -> Option<&mut CallRt> {
        self.calls.iter_mut().find(|c| c.id == id)
    }

    pub fn local_hostport(&self) -> String {
        match self.cfg.local_ip {
            std::net::IpAddr::V4(a) => format!("{a}:{}", self.local_port),
            std::net::IpAddr::V6(a) => format!("[{a}]:{}", self.local_port),
        }
    }

    fn local_addr(&self) -> SipAddr {
        SipAddr::from(std::net::SocketAddr::new(self.cfg.local_ip, self.local_port))
    }

    /// The port peers reach us on over `t`: the listener's for a stream
    /// transport we listen on, else the UDP port.
    fn port_for(&self, t: Transport) -> u16 {
        match t {
            Transport::Tcp => self.tcp_listening,
            Transport::Tls => self.tls_listening,
            Transport::Udp => None,
        }
        .unwrap_or(self.local_port)
    }

    /// Our Contact URI for an account: the stack's NAT-aware form (the
    /// address the registrar saw, when it differs from ours).
    pub fn contact_uri(&self, user: &str, public: Option<rsipstack::sip::HostWithPort>, t: Transport) -> Uri {
        let local = SipAddr::from(std::net::SocketAddr::new(self.cfg.local_ip, self.port_for(t)));
        let mut uri = Registration::create_nat_aware_contact(user, public, &local).uri;
        if t != Transport::Udp {
            uri.params.push(rsipstack::sip::Param::Transport(sip_transport(t)));
        }
        uri
    }

    pub fn account_contact(&self, a: &Acct) -> Uri {
        self.contact_uri(&a.cfg.user, a.public.clone(), a.cfg.host(a.alt).transport)
    }

    pub fn credential(a: &Acct) -> Credential {
        let (auth_user, password) = a.cfg.credentials(a.alt);
        Credential { username: a.cfg.user.clone(), password, realm: None, auth_username: Some(auth_user) }
    }

    /// `sip:server[:port][;transport=x]` of an account's current host, with
    /// an optional user part.  The port is written whenever it is not the
    /// transport's default, because the stack routes by the URI alone.
    pub fn host_uri(a: &Acct, user: Option<&str>) -> String {
        let h = a.cfg.host(a.alt);
        let default = if h.transport == Transport::Tls { 5061 } else { 5060 };
        let port = if h.effective_port() == default { String::new() } else { format!(":{}", h.effective_port()) };
        let user = user.map(|u| format!("{u}@")).unwrap_or_default();
        format!("sip:{user}{}{port}{}", h.server, transport_param(h.transport))
    }

    /// The transport address for `host:port` over `t`.
    pub fn addr(host: &str, port: u16, t: Transport) -> Result<SipAddr, String> {
        use rsipstack::sip::{Host, HostWithPort};
        let host = Host::try_from(host.trim_matches(|c| c == '[' || c == ']'))
            .map_err(|e| format!("bad host {host}: {e}"))?;
        Ok(SipAddr { r#type: Some(sip_transport(t)), addr: HostWithPort { host, port: Some(port.into()) } })
    }

    /// Where a new request for `uri` goes: the outbound proxy when one is
    /// configured, else wherever the URI itself points (RFC 3261 8.1.2).
    pub fn request_dest(&self, a: &Acct, uri: &str) -> Result<(SipAddr, Transport), String> {
        let h = a.cfg.host(a.alt);
        if !a.cfg.outbound_proxy.is_empty() {
            let u = SipUri::parse(&format!("sip:{}", a.cfg.outbound_proxy)).ok_or("bad outbound proxy")?;
            let port = u.port.unwrap_or(if h.transport == Transport::Tls { 5061 } else { 5060 });
            return Ok((Self::addr(&u.host, port, h.transport)?, h.transport));
        }
        let u = SipUri::parse(uri).ok_or_else(|| format!("bad URI {uri}"))?;
        let t = match u.param("transport").map(str::to_ascii_lowercase).as_deref() {
            Some("tcp") => Transport::Tcp,
            Some("tls") => Transport::Tls,
            Some("udp") => Transport::Udp,
            _ if u.secure => Transport::Tls,
            _ => h.transport,
        };
        let default = if t == Transport::Tls { 5061 } else { 5060 };
        let port = match u.port {
            Some(p) => p,
            None if u.host.eq_ignore_ascii_case(&h.server) && t == h.transport => h.effective_port(),
            None => default,
        };
        Ok((Self::addr(&u.host, port, t)?, t))
    }

    // ------------------------------------- requests outside any dialog

    /// Builds a request that belongs to no dialog (MESSAGE, OPTIONS,
    /// PUBLISH, and the seed of a subscription).  The dialog layer has no
    /// call for these, so they ride a plain transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn make_request(
        &self,
        method: rsipstack::sip::Method,
        target: &str,
        from_uri: &str,
        from_tag: &str,
        to: &str,
        to_tag: Option<&str>,
        call_id: &str,
        cseq: u32,
        transport: Transport,
        contact: Option<Uri>,
        headers: Vec<Header>,
        body: Vec<u8>,
    ) -> Result<Request, String> {
        let mut via_addr = self.local_addr();
        via_addr.r#type = Some(sip_transport(transport));
        let via = self.inner.get_via(Some(via_addr), None).map_err(|e| e.to_string())?;
        let from = rsipstack::sip::typed::From { display_name: None, uri: uri(from_uri)?, params: vec![] }
            .with_tag(from_tag.into());
        let mut to = rsipstack::sip::typed::To { display_name: None, uri: uri(to)?, params: vec![] };
        if let Some(tag) = to_tag {
            to = to.with_tag(tag.into());
        }
        let mut req =
            self.inner.make_request(method, uri(target)?, via, from, to, cseq, Some(call_id.to_string().into()));
        if let Some(c) = contact {
            req.headers.push(rsipstack::sip::typed::Contact { display_name: None, uri: c, params: vec![] }.into());
        }
        for h in headers {
            req.headers.push(h);
        }
        req.body = body;
        Ok(req)
    }

    /// Runs one out-of-dialog client transaction, answering a digest
    /// challenge once with the stack's authenticator.
    pub async fn transact(
        inner: EndpointInnerRef,
        req: Request,
        dest: SipAddr,
        cred: Option<Credential>,
    ) -> Result<Response, String> {
        let mut seq = req.cseq_header().ok().and_then(|c| c.seq().ok()).unwrap_or(1);
        let key = TransactionKey::from_request(&req, TransactionRole::Client).map_err(|e| e.to_string())?;
        let mut tx = Transaction::new_client(key, req, inner, None);
        tx.destination = Some(dest.clone());
        tx.send().await.map_err(|e| e.to_string())?;
        let mut answered = false;
        while let Some(msg) = tx.receive().await {
            let rsipstack::sip::SipMessage::Response(resp) = msg else { continue };
            match resp.status_code.code() {
                0..=199 => {}
                401 | 407 if !answered && cred.is_some() => {
                    answered = true;
                    seq += 1;
                    let cred = cred.as_ref().expect("checked");
                    tx = handle_client_authenticate(seq, &tx, resp, cred).await.map_err(|e| e.to_string())?;
                    tx.destination = Some(dest.clone());
                    tx.send().await.map_err(|e| e.to_string())?;
                }
                _ => return Ok(resp),
            }
        }
        Err("no answer".into())
    }

    // ------------------------------------------------------- server side

    /// Answers a transaction that belongs to no dialog and keeps it alive
    /// until it has absorbed retransmissions (and the ACK of an INVITE).
    pub fn reply(&self, mut tx: Transaction, code: u16, headers: Vec<Header>) {
        tokio::spawn(async move {
            if let Err(e) = tx.reply_with(StatusCode::from(code), headers, None).await {
                log::debug!("sip: could not answer: {e}");
            }
            if tx.original.method == rsipstack::sip::Method::Invite {
                while tx.receive().await.is_some() {}
            }
        });
    }

    /// Answers an in-dialog request the layer surfaced with a handle.
    pub fn respond(&self, handle: TransactionHandle, code: u16, headers: Vec<Header>, body: Option<Vec<u8>>) {
        tokio::spawn(async move {
            let headers = (!headers.is_empty()).then_some(headers);
            if handle.respond(StatusCode::from(code), headers, body).await.is_err() {
                log::debug!("sip: the request was gone before it could be answered");
            }
        });
    }

    /// Lets the layer run a request that matched one of its dialogs.
    pub fn dispatch(&self, mut dialog: rsipstack::dialog::dialog::Dialog, mut tx: Transaction) {
        tokio::spawn(async move {
            if let Err(e) = dialog.handle(&mut tx).await {
                log::debug!("sip: dialog request failed: {e}");
            }
            // A server INVITE transaction still has to see its ACK (or
            // time out); dropping it now would cancel its timers.
            if tx.original.method == rsipstack::sip::Method::Invite {
                while tx.receive().await.is_some() {}
            }
        });
    }

    // ------------------------------------------------------------- state

    /// Recomputes line keys, rapid-dial keys, and the message lamp.
    pub fn sync_keys(&self) {
        let model = self.cfg.model;
        let calls = self.store.get::<opendeskphone_state::Calls>();
        let presence = self.store.get::<opendeskphone_state::Presence>();
        let mwi = self.store.get::<opendeskphone_state::Mwi>();
        let mut line_keys = Vec::new();
        for i in 0..model.lines() {
            let id = AccountId(i);
            let mut key = LineKey { index: i, ..Default::default() };
            if let Some(a) = self.acct(id) {
                key.binding = LineKeyBinding::Account(id);
                key.label = if a.cfg.label.is_empty() { a.cfg.user.clone() } else { a.cfg.label.clone() };
                let mine: Vec<_> = calls.visible().filter(|c| c.account == id).collect();
                key.calls = mine.iter().map(|c| c.id).collect();
                use opendeskphone_state::CallState as S;
                key.lamp = if mine.iter().any(|c| c.state == S::Ringing) {
                    LampState::blinking(LampColor::Green, Blink::Fast)
                } else if mine.iter().any(|c| {
                    matches!(c.state, S::Active | S::Dialing | S::Proceeding | S::Ringback { .. } | S::Transferring)
                }) {
                    LampState::solid(LampColor::Green)
                } else if mine.iter().any(|c| c.state.is_held()) {
                    LampState::blinking(LampColor::Red, Blink::Slow)
                } else {
                    LampState::OFF
                };
            }
            line_keys.push(key);
        }
        // The BLF and rapid-dial items, where stock puts them.
        let layout = crate::keys::layout(model, self.accts.len(), self.cfg.page_line_keys, &self.cfg.blf);
        let lamp_of = |entry: &str| presence.get(entry).map_or(LampState::OFF, |e| e.state.lamp());
        let pages = layout.pages;
        let mut rapid = Vec::new();
        for (page, slots) in layout.side.iter().take(pages as usize).enumerate() {
            for (index, slot) in slots.iter().enumerate() {
                if let crate::keys::Slot::Item(i) = slot {
                    let b = &self.cfg.blf[*i];
                    rapid.push(RapidDialKey {
                        index: index as u8,
                        page: page as u8,
                        entry: b.id.clone(),
                        label: b.label.clone(),
                        number: b.number.clone(),
                        lamp: lamp_of(&b.id),
                    });
                }
            }
        }
        // Line keys the accounts leave free, page by page.
        let line_pages: Vec<Vec<LineKey>> = layout
            .line
            .iter()
            .take(pages as usize)
            .map(|slots| {
                let mut keys = line_keys.clone();
                for (key, slot) in keys.iter_mut().zip(slots) {
                    if let (LineKeyBinding::Unused, crate::keys::Slot::Item(i)) = (&key.binding, slot) {
                        let b = &self.cfg.blf[*i];
                        key.binding = LineKeyBinding::Blf(b.id.clone());
                        key.label = b.label.clone();
                        key.lamp = lamp_of(&b.id);
                    }
                }
                keys
            })
            .collect();
        // The message lamp is stock's activity light: the strongest of
        // new voicemail, an alerting call, and a call in progress.  Held
        // calls count for nothing.
        let message_lamp = {
            use opendeskphone_state::CallState as S;
            let act = &self.cfg.activity;
            if calls.visible().any(|c| {
                matches!(c.state, S::Active | S::Dialing | S::Proceeding | S::Ringback { .. } | S::Transferring)
            }) {
                act.active
            } else if calls.visible().any(|c| c.state == S::Ringing) {
                act.ringing
            } else if mwi.any_waiting() {
                act.new_vm
            } else {
                LampState::OFF
            }
        };
        self.store.update::<Keys, _>(|k| {
            if k.rapid_page >= pages.max(1) {
                k.rapid_page = 0;
            }
            k.line_keys = line_pages.get(k.rapid_page as usize).cloned().unwrap_or(line_keys);
            k.rapid_dial = rapid;
            k.rapid_pages = pages;
            k.message_lamp = message_lamp;
        });
    }

    /// The server event name a request carries.  Stock's app engine takes
    /// it from `X-Digium-Event-Name`, `X-Event-Name`, or `X-Event-Type`
    /// (the last one present wins).
    pub fn event_name(req: &Request) -> Option<String> {
        req.headers
            .iter()
            .filter(|h| ["x-digium-event-name", "x-event-name", "x-event-type"].iter().any(|n| h.name().eq_ignore_ascii_case(n)))
            .map(|h| h.value().trim().to_string())
            .filter(|v| !v.is_empty())
            .last()
    }

    /// What the configuration says to do with an incoming call by the
    /// event name it carries.  Stock's phone side reads the name from
    /// `X-Event-Name`, `X-Digium-Event-Name`, or
    /// `X-Digium-AppServer-Event-Name` (not from `X-Event-Type`).
    pub fn event_action(&self, req: &Request) -> Option<crate::config::EventAction> {
        ["X-Event-Name", "X-Digium-Event-Name", "X-Digium-AppServer-Event-Name"]
            .iter()
            .filter_map(|n| util::header(&req.headers, n))
            .find_map(|name| self.cfg.events.iter().find(|(id, _)| id == name.trim()).map(|(_, a)| *a))
    }

    /// A server event carried on a call's INVITE or on an INFO in its
    /// dialog, passed on for apps.
    pub fn emit_push(&self, req: &Request, method: &str, call: opendeskphone_state::CallId) {
        let Some(name) = Self::event_name(req) else { return };
        let kind = util::header(&req.headers, "X-Event-Type").unwrap_or("").trim();
        let sdp = util::header(&req.headers, "Content-Type").is_some_and(|c| c.to_ascii_lowercase().contains("sdp"));
        let data = match util::header(&req.headers, "X-Event-Data") {
            Some(d) => d.trim().to_string(),
            None if !sdp => String::from_utf8_lossy(&req.body).into_owned(),
            None => String::new(),
        };
        log::info!("sip: event {name} on {method} for call {}", call.0);
        self.store.emit(opendeskphone_state::Event::CallPush {
            name,
            kind: kind.to_string(),
            data,
            method: method.to_string(),
            call,
        });
    }

    pub fn heartbeat(&self) {
        self.store.heartbeat(Subsystem::Sip);
    }

    /// The account an incoming out-of-dialog request is for: the one whose
    /// user matches the request URI or To, else the first.
    pub fn account_for(&self, req: &Request) -> Option<AccountId> {
        let ruri_user = SipUri::parse(&req.uri.to_string()).map(|u| u.user).unwrap_or_default();
        let to_user = util::header(&req.headers, "To").map(|t| NameAddr::parse(t).user()).unwrap_or_default();
        self.accts
            .iter()
            .find(|a| a.cfg.user == ruri_user)
            .or_else(|| self.accts.iter().find(|a| a.cfg.user == to_user))
            .or(self.accts.first())
            .map(|a| a.cfg.id)
    }
}

/// Notes CANCELs that carry `Reason: ...;text="Call completed elsewhere"`.
/// The dialog layer ends the call on a CANCEL without showing the request,
/// so this is where its Reason can be seen.
pub(crate) struct CancelWatch(pub Arc<std::sync::Mutex<Vec<String>>>);

impl rsipstack::transaction::endpoint::MessageInspector for CancelWatch {
    fn before_send(&self, msg: rsipstack::sip::SipMessage, _dest: Option<&SipAddr>) -> rsipstack::sip::SipMessage {
        msg
    }

    fn after_received(&self, msg: rsipstack::sip::SipMessage, _from: Option<&SipAddr>) -> rsipstack::sip::SipMessage {
        if let rsipstack::sip::SipMessage::Request(req) = &msg {
            if req.method == rsipstack::sip::Method::Cancel
                && util::header(&req.headers, "Reason").is_some_and(|r| r.to_ascii_lowercase().contains("elsewhere"))
            {
                if let Some(id) = util::header(&req.headers, "Call-ID") {
                    let mut list = self.0.lock().unwrap();
                    if list.len() >= 16 {
                        list.remove(0);
                    }
                    list.push(id.to_string());
                }
            }
        }
        msg
    }
}

/// Forgets our TCP connections when they close, so the next REGISTER
/// opens (and marks) a new one.
pub(crate) struct StreamWatch(pub Arc<std::sync::Mutex<std::collections::HashSet<String>>>);

#[async_trait::async_trait]
impl rsipstack::transaction::endpoint::TransportEventInspector for StreamWatch {
    async fn handle(&self, event: rsipstack::transport::TransportEvent) -> Option<rsipstack::transport::TransportEvent> {
        if let rsipstack::transport::TransportEvent::Closed(conn) = &event {
            if let Some(addr) = conn.get_remote_addr() {
                self.0.lock().unwrap().remove(&addr.to_string());
            }
        }
        Some(event)
    }
}
