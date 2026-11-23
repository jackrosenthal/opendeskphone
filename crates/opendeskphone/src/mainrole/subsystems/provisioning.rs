//! What the DPMA client needs from the rest of the main role beyond the
//! configuration sink in `telephony`: the files a provisioned configuration
//! names, the "ignore provisioned firmware" setting as a fetched
//! configuration would leave it, and the discovery inputs (DHCP option 66
//! and mDNS), kept current.

use std::sync::Arc;
use std::time::Duration;

use opendeskphone_config::{Config, OpenDeskPhoneSettings, Element, Nvdata};
use opendeskphone_dpma::DpmaHandle;
use opendeskphone_dpma::resources::{Files, Kind, PushedApp, Resource, Wanted};
use opendeskphone_state::{DiscoveredServer, DiscoverySource, ProvisionPhase, Provisioning, Topic};
use log::info;

use super::net::NetHandle;
use crate::mainrole::Context;

/// The files the merged configuration names, as stock reads them:
/// `<contacts id url md5>`, `<ringtones><tones><tone id url md5>`,
/// `<smart_blf><blf_items url md5>`, `<smart_blf><datasets url md5>`,
/// `<setting id="wallpaper_file" url md5>` with this firmware's
/// `<setting id="opendeskphone_palette_file" url md5>` (the palette for that
/// picture: fetched only when the picture is named, and on a model with a
/// color screen), and, for the log only, the app packages of
/// `<appconfig id><url url/>`.
pub fn wanted_resources(cfg: &Config) -> Wanted {
    let mut wanted = Wanted::default();
    for c in cfg.contact_sources() {
        wanted.resources.push(Resource { kind: Kind::ContactList, id: c.id, url: c.url, md5: c.md5 });
    }
    for t in cfg.ringtones().tones {
        if let Some(url) = t.url {
            wanted.resources.push(Resource { kind: Kind::Ringtone, id: t.id, url, md5: t.md5 });
        }
    }
    let root = cfg.root();
    if let Some(smart) = root.child("smart_blf") {
        for (name, kind) in [("blf_items", Kind::BlfItems), ("datasets", Kind::Datasets)] {
            for e in smart.children_named(name) {
                if let Some(url) = e.attr("url").filter(|u| !u.is_empty()) {
                    wanted.resources.push(Resource {
                        kind,
                        id: String::new(),
                        url: url.to_string(),
                        md5: e.attr("md5").map(str::to_string),
                    });
                }
            }
        }
    }
    // Stock sends the wallpaper setting to every model; only a color
    // screen shows one.
    if cfg.model().has_themes() {
        use opendeskphone_config::palette::{PALETTE_SETTING, WALLPAPER_SETTING};
        let named = |id: &str, kind: Kind| {
            let url = cfg.setting_attr(id, "url").map(str::trim).filter(|u| !u.is_empty())?;
            Some(Resource { kind, id: String::new(), url: url.to_string(), md5: cfg.setting_attr(id, "md5").map(str::to_string) })
        };
        if let Some(picture) = named(WALLPAPER_SETTING, Kind::Wallpaper) {
            wanted.resources.push(picture);
            wanted.resources.extend(named(PALETTE_SETTING, Kind::Palette));
        }
    }
    for app in root.children_named("appconfig") {
        let url = app.child("url").and_then(|u| u.attr("url")).filter(|u| !u.is_empty());
        if let (Some(id), Some(url)) = (app.attr("id").filter(|i| !i.is_empty()), url) {
            wanted.apps.push(PushedApp { id: id.to_string(), url: url.to_string() });
        }
    }
    wanted
}

/// Whether provisioned firmware is ignored once `fetched` (a fetched
/// configuration) is in effect, given the setting stored on the phone.
/// `None` when the document does not parse: the value in effect stays.
pub fn ignores_firmware(fetched: &[u8], model: opendeskphone_config::Model, local: &OpenDeskPhoneSettings) -> Option<bool> {
    let root = Element::parse(std::str::from_utf8(fetched).ok()?).ok()?;
    Some(Config::from_merged(root, model, None).with_opendeskphone_settings(local.clone()).ignore_provisioned_firmware().value)
}

/// nvdata, for the resource pass.
pub struct NvFiles(pub Nvdata);

impl Files for NvFiles {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        self.0.read(rel).ok().flatten()
    }
    fn write(&self, rel: &str, data: &[u8]) -> Result<(), String> {
        if let Some(dir) = self.0.path(rel).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        self.0.write(rel, data).map_err(|e| e.to_string())
    }
    fn remove(&self, rel: &str) {
        let _ = self.0.remove(rel);
    }
    fn list(&self, dir: &str) -> Vec<String> {
        self.0.list(dir).unwrap_or_default()
    }
}

/// The download client: names are resolved by the network manager's
/// resolver (the one DHCP feeds), or by the host system's when the manager
/// has none (the simulator).
fn downloader(net: &NetHandle) -> opendeskphone_dpma::fetch::HttpSource {
    use std::net::{SocketAddr, ToSocketAddrs};
    let (runtime, net) = (tokio::runtime::Handle::current(), net.clone());
    opendeskphone_dpma::fetch::HttpSource::with_lookup(Box::new(move |host, port| {
        let Some(resolver) = net.resolver() else {
            return (host, port).to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("no address"));
        };
        let addrs = runtime.block_on(resolver.lookup_ip(host, false, false)).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        addrs.first().map(|ip| SocketAddr::new(*ip, port)).ok_or_else(|| std::io::Error::other("no address"))
    }))
}

/// The HTTP client for what a configuration names by URL: firmware images,
/// contact lists, and ringtones.  An `https://` server is checked against
/// the trust bundle ([`super::trust::roots`]) unless `allow_insecure_ssl`
/// is set: the policy of SIP over TLS and of the apps' requests.  (Stock
/// fetches firmware with `curl --insecure`; we check that server too.)
/// Call it on the runtime; the downloads themselves run on blocking threads.
pub fn http_source(net: &NetHandle, config: super::SharedConfig) -> Arc<dyn opendeskphone_dpma::firmware::FirmwareSource> {
    use opendeskphone_dpma::fetch::Trust;
    Arc::new(downloader(net).with_trust(Box::new(move || {
        let store = config.lock().unwrap();
        let cfg = store.config();
        if cfg.sip().allow_insecure_ssl { Trust::Any } else { Trust::Roots(super::trust::roots(store.nvdata(), &cfg)) }
    })))
}

/// A `_digiumproxy._udp` announcement as a provisioning server: the TXT
/// record's `sipUrl` when it has one, else the address and port.
fn announced(s: &opendeskphone_net::mdns::Service) -> Option<DiscoveredServer> {
    let sip_url = s.txt.iter().find(|(k, v)| k.eq_ignore_ascii_case("sipUrl") && v.starts_with("sip")).map(|(_, v)| v.clone());
    let address = match sip_url {
        Some(url) => url,
        None => {
            let host = s.addrs.first().map(|a| a.to_string()).or_else(|| (!s.host.is_empty()).then(|| s.host.clone()))?;
            format!("{host}:{}", s.port)
        }
    };
    Some(DiscoveredServer { source: DiscoverySource::Mdns, name: s.instance.clone(), address })
}

/// How long one mDNS browse listens.
const BROWSE_WINDOW: Duration = Duration::from_secs(3);
/// How often the phone browses again while it has no server.  (Stock keeps
/// a browser running and hears announcements as they come.)
const BROWSE_EVERY: Duration = Duration::from_secs(30);

/// Keeps the DPMA client's discovery inputs current: whether the network
/// is up, DHCP option 66 (whenever it changes, not only with the address),
/// and the servers announced over mDNS.  The network is browsed when it
/// comes up and, while the phone waits for a server, every
/// [`BROWSE_EVERY`]; the last result is passed along with every report.
///
/// Another address is another network to the client: it is told that the
/// network went down and came up, whether or not the time without an
/// address was seen here, so the session is made anew from the address the
/// phone has now.  (The network subsystem takes the new session as the
/// server's answer at a static address the server gave.)
pub async fn feed_discovery(ctx: Context, net: NetHandle, dpma: DpmaHandle) {
    let sub = ctx.store.subscribe([Topic::Network, Topic::Provisioning]);
    let mut mdns: Vec<DiscoveredServer> = Vec::new();
    let mut told: Option<(bool, Option<String>, Vec<DiscoveredServer>)> = None;
    let mut told_address: Option<std::net::Ipv4Addr> = None;
    let mut was_up = false;
    let mut browse_at = tokio::time::Instant::now();
    loop {
        let (address, option66) = if ctx.sim {
            (None, None)
        } else {
            ctx.store.read::<opendeskphone_state::Network, _>(|n| (n.ipv4.address, Some(n.dhcp_tftp_server.clone()).filter(|s| !s.is_empty())))
        };
        let up = ctx.sim || address.is_some();
        let waiting = ctx.store.read::<Provisioning, _>(|p| p.phase == ProvisionPhase::AwaitingServer);
        let now = tokio::time::Instant::now();
        let browse = up && !ctx.sim && (!was_up || (waiting && now >= browse_at));
        was_up = up;
        let report = (up, option66, mdns.clone());
        if up && address != told_address && told.as_ref().is_some_and(|t| t.0) {
            info!("dpma: the phone's address changed; the session starts over");
            dpma.network(false, report.1.clone(), report.2.clone());
            told = None;
        }
        told_address = address;
        if told.as_ref() != Some(&report) {
            dpma.network(report.0, report.1.clone(), report.2.clone());
            told = Some(report);
        }
        if browse {
            let mut found: Vec<DiscoveredServer> = net.browse_proxies(BROWSE_WINDOW).await.iter().filter_map(announced).collect();
            found.sort_by(|a, b| (&a.name, &a.address).cmp(&(&b.name, &b.address)));
            found.dedup();
            if found != mdns {
                info!("dpma: {} server(s) announced over mDNS", found.len());
                mdns = found;
            }
            browse_at = tokio::time::Instant::now() + BROWSE_EVERY;
            // The store may have moved on during the browse.
            continue;
        }
        tokio::select! {
            _ = sub.changed() => {}
            _ = tokio::time::sleep_until(browse_at), if waiting && up && !ctx.sim => {}
        }
    }
}

/// Whether the phone starts in its setup (`Phase::Provisioning`: the
/// display shows the provisioning screens, not the idle screen): it has no
/// saved provisioning server (`configurator.xml`) and no SIP accounts, as
/// after a factory reset.  This is stock's condition for its `fresh` state
/// (`stateMachine.js`, `StateMachine.start`).
///
/// It is decided from nvdata when start-up ends, not from the DPMA client:
/// the client starts only once the phone has an address, and until then
/// (or for good, with no network) the phone would show an idle screen it
/// cannot place a call from.
pub fn starts_in_setup(cfg: &Config, configurator: Option<&[u8]>) -> bool {
    let saved = configurator.map(opendeskphone_dpma::xml::Configurator::parse).unwrap_or_default();
    saved.saved_server().is_none() && cfg.accounts().is_empty()
}

/// The phase the phone is in, given the one it was in, the DPMA client's,
/// and the number of SIP accounts.
///
/// Setup begins at the start ([`starts_in_setup`]) or when the client comes
/// to wait for a server on a phone without accounts (it waits only when no
/// server is saved, e.g., after "Forget server").  It ends when the phone
/// is provisioned or the user chose to go on without a server.  A phone
/// with a saved server or with accounts of its own never enters it.
pub fn system_phase(now: opendeskphone_state::Phase, provisioning: &ProvisionPhase, accounts: usize) -> opendeskphone_state::Phase {
    use opendeskphone_state::Phase;
    match (now, provisioning) {
        (Phase::Running, ProvisionPhase::AwaitingServer) if accounts == 0 => Phase::Provisioning,
        (Phase::Provisioning, ProvisionPhase::Provisioned | ProvisionPhase::Standalone) => Phase::Running,
        (now, _) => now,
    }
}

/// Keeps `System::phase` at [`system_phase`].  Runs from the start of the
/// main role, whether or not the DPMA client ever starts.
pub async fn follow_setup(store: opendeskphone_state::Store) {
    let sub = store.subscribe([Topic::System, Topic::Provisioning, Topic::Accounts]);
    loop {
        let provisioning = store.read::<Provisioning, _>(|p| p.phase.clone());
        let accounts = store.read::<opendeskphone_state::Accounts, _>(|a| a.accounts.len());
        let now = store.read::<opendeskphone_state::System, _>(|s| s.phase);
        let next = system_phase(now, &provisioning, accounts);
        if next != now {
            if next == opendeskphone_state::Phase::Provisioning {
                info!("setup: no saved server and no accounts; showing the server menu");
            } else {
                info!("setup: over ({provisioning:?})");
            }
            // Only the step that was decided on: a shutdown that began in
            // between stays.
            store.update::<opendeskphone_state::System, _>(|s| {
                if s.phase == now {
                    s.phase = next;
                }
            });
        }
        sub.changed().await;
    }
}

/// What the user chose on the setup screens before the DPMA client existed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EarlyChoice {
    /// The server address entered ("Enter address...").
    pub server: Option<String>,
    /// "Use without a server".
    pub standalone: bool,
}

impl EarlyChoice {
    /// Takes a provisioning command the way the client would, as far as
    /// that can be done without it: a server is remembered for the client,
    /// and going on without a server takes effect at once.  Returns the
    /// phase to show, if it changes, and the reply.
    pub fn take(&mut self, command: &opendeskphone_state::Command) -> (Option<ProvisionPhase>, Result<opendeskphone_state::Reply, opendeskphone_state::CommandError>) {
        use opendeskphone_state::{Command, CommandError, Reply};
        match command {
            Command::StartProvisioning { server } => {
                if let Some(s) = server.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    self.server = Some(s.to_string());
                }
                self.standalone = false;
                (Some(ProvisionPhase::Idle), Ok(Reply::Done))
            }
            Command::Reprovision => {
                self.standalone = false;
                (Some(ProvisionPhase::Idle), Ok(Reply::Done))
            }
            Command::SkipProvisioning => {
                self.standalone = true;
                (Some(ProvisionPhase::Standalone), Ok(Reply::Done))
            }
            _ => (None, Err(CommandError::Failed("provisioning has not started: the phone has no network address yet".into()))),
        }
    }

    /// The commands that tell the client, once it exists, what was chosen.
    pub fn replay(&self) -> Vec<opendeskphone_state::Command> {
        use opendeskphone_state::Command;
        let mut v = Vec::new();
        if let Some(server) = &self.server {
            v.push(Command::StartProvisioning { server: Some(server.clone()) });
        }
        if self.standalone {
            v.push(Command::SkipProvisioning);
        }
        v
    }
}

/// The provisioning commands' handler until the DPMA client takes them
/// over.  The client starts only once the phone has an address; a phone in
/// its setup with no network must still let its user go on without a
/// server or enter one.
pub struct EarlyCommands {
    choice: Arc<std::sync::Mutex<EarlyChoice>>,
    task: tokio::task::JoinHandle<()>,
}

/// Registers the early handler.  Call it before the client can start.
pub fn early_commands(store: &opendeskphone_state::Store) -> EarlyCommands {
    let rx = store.handle(opendeskphone_state::Domain::Provisioning);
    let choice = Arc::new(std::sync::Mutex::new(EarlyChoice::default()));
    let (store, shared) = (store.clone(), choice.clone());
    let task = tokio::spawn(async move {
        while let Some(envelope) = rx.next().await {
            let (command, responder) = envelope.split();
            let (phase, reply) = {
                let mut choice = shared.lock().unwrap();
                let r = choice.take(&command);
                let server = choice.server.clone();
                if r.0.is_some() {
                    store.update::<Provisioning, _>(|p| {
                        p.phase = r.0.clone().unwrap();
                        p.server = server.map(|s| DiscoveredServer { source: DiscoverySource::Manual, name: s.clone(), address: s });
                    });
                }
                r
            };
            if let Some(phase) = phase {
                info!("setup: before the provisioning client: {command:?} ({phase:?})");
            }
            responder.reply(reply);
        }
    });
    EarlyCommands { choice, task }
}

impl EarlyCommands {
    /// The client has registered itself for the commands: stop, and tell
    /// it what the user chose meanwhile.  Call it right after the client
    /// starts and before it hears of the network, so that a phone used
    /// without a server does not pass through the server menu again.
    pub fn hand_over(self, store: &opendeskphone_state::Store) {
        self.task.abort();
        for command in self.choice.lock().unwrap().replay() {
            if !store.post(command) {
                log::warn!("setup: the provisioning client did not take the choice made before it started");
            }
        }
    }
}

/// Starts [`feed_discovery`].
pub fn start_discovery(ctx: &Context, net: &NetHandle, dpma: &DpmaHandle) {
    tokio::spawn(feed_discovery(ctx.clone(), net.clone(), dpma.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(xml: &str) -> Config {
        Config::from_merged(Element::parse(xml).unwrap(), opendeskphone_config::Model::D60, None)
    }

    #[test]
    fn resources_of_a_provisioned_configuration() {
        // The shapes `res_open_dpma` builds.
        let cfg = config(
            r#"<config>
<appconfig id="contacts"><account id="100"/></appconfig>
<appconfig id="queues" network_id="n"><url url="file://queues.zip" md5="aa" /><auto_start/></appconfig>
<contacts url="file://team.xml" id="0" />
<contacts id="1" url="http://pbx/files/all.xml" md5="0123456789abcdef0123456789abcdef" network_id="n" />
<ringtones>
<tones>
<tone id="Digium" display="Digium" type="phone" />
<tone id="Chimes" display="Chimes" url="file://chimes.sln" type="user" network_id="n" />
</tones>
</ringtones>
<smart_blf>
<blf_items url="file://blf.xml" network_id="n"/>
<datasets url="http://pbx/files/data.xml" md5="ff"/>
</smart_blf>
</config>"#,
        );
        let w = wanted_resources(&cfg);
        let r = |kind, id: &str, url: &str, md5: Option<&str>| Resource { kind, id: id.into(), url: url.into(), md5: md5.map(str::to_string) };
        assert_eq!(
            w.resources,
            vec![
                r(Kind::ContactList, "0", "file://team.xml", None),
                r(Kind::ContactList, "1", "http://pbx/files/all.xml", Some("0123456789abcdef0123456789abcdef")),
                r(Kind::Ringtone, "Chimes", "file://chimes.sln", None),
                r(Kind::BlfItems, "", "file://blf.xml", None),
                r(Kind::Datasets, "", "http://pbx/files/data.xml", Some("ff")),
            ]
        );
        // The built-in app's configuration is not a package; the pushed
        // one is, and is only ever logged.
        assert_eq!(w.apps, vec![PushedApp { id: "queues".into(), url: "file://queues.zip".into() }]);
        assert_eq!(wanted_resources(&config("<config/>")), Wanted::default());
    }

    #[test]
    fn the_wallpaper_picture_and_its_palette_file_are_resources() {
        // The shapes `res_open_dpma` builds (`wallpaper_file`, and
        // `opendeskphone_palette_file` beside it).
        let xml = |extra: &str| {
            format!(
                r#"<config>
<setting id="wallpaper_file" value="user" url="file://wall.png" path="/user_wallpaper.png" conflict="freeze" />
{extra}</config>"#
            )
        };
        let palette = r#"<setting id="opendeskphone_palette_file" value="user" url="http://pbx/files/wall.txtpb" md5="ab" path="/wallpaper_palette.txtpb" conflict="freeze" />"#;
        let on = |model, xml: &str| wanted_resources(&Config::from_merged(Element::parse(xml).unwrap(), model, None)).resources;
        let r = |kind, url: &str, md5: Option<&str>| Resource { kind, id: String::new(), url: url.into(), md5: md5.map(str::to_string) };
        let d65 = opendeskphone_config::Model::D65;
        assert_eq!(on(d65, &xml("")), vec![r(Kind::Wallpaper, "file://wall.png", None)]);
        assert_eq!(
            on(d65, &xml(palette)),
            vec![r(Kind::Wallpaper, "file://wall.png", None), r(Kind::Palette, "http://pbx/files/wall.txtpb", Some("ab"))]
        );
        // A palette file without a picture is not fetched; neither is
        // anything on a model with a gray screen.
        assert_eq!(on(d65, &format!("<config>{palette}</config>")), vec![]);
        assert_eq!(on(opendeskphone_config::Model::D50, &xml(palette)), vec![]);
        // A setting without a URL names nothing.
        assert_eq!(on(d65, r#"<config><setting id="wallpaper_file" value="user" url=" "/></config>"#), vec![]);
    }

    #[test]
    fn ignore_setting_of_a_fetched_configuration() {
        let model = opendeskphone_config::Model::D60;
        let unset = OpenDeskPhoneSettings::default();
        let on = br#"<config><setting id="opendeskphone_ignore_provisioned_firmware" value="1" /><firmwares><firmware model="D60" version="2_9_28" url="http://pbx/f.eff"/></firmwares></config>"#;
        let off = br#"<config><setting id="opendeskphone_ignore_provisioned_firmware" value="0" /></config>"#;
        let silent = br#"<config><setting id="login_password" value="789" /></config>"#;
        assert_eq!(ignores_firmware(on, model, &unset), Some(true));
        assert_eq!(ignores_firmware(off, model, &unset), Some(false));
        assert_eq!(ignores_firmware(silent, model, &unset), Some(false), "off by default");
        // The value stored on the phone outranks an ordinary provisioned
        // one, as for every Open Desk Phone setting.
        let local_on = OpenDeskPhoneSettings { ignore_provisioned_firmware: Some(true), ..Default::default() };
        let local_off = OpenDeskPhoneSettings { ignore_provisioned_firmware: Some(false), ..Default::default() };
        assert_eq!(ignores_firmware(silent, model, &local_on), Some(true));
        assert_eq!(ignores_firmware(on, model, &local_off), Some(false));
        assert_eq!(ignores_firmware(b"<config", model, &unset), None);
    }

    #[test]
    fn nvdata_as_resource_files() {
        let dir = std::env::temp_dir().join(format!("opendeskphone-provisioning-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nv = Nvdata::new(&dir);
        let f = NvFiles(nv.clone());
        // The directories need not exist yet.
        f.write("contact_lists/0", b"<contacts/>").unwrap();
        f.write("md5s/0", b"0123456789abcdef0123456789abcdef").unwrap();
        f.write("blf_items.xml", b"<config/>").unwrap();
        assert_eq!(f.read("contact_lists/0").as_deref(), Some(&b"<contacts/>"[..]));
        assert_eq!(f.list("contact_lists"), vec!["0"]);
        // The same files through the names the rest of the firmware uses.
        assert_eq!(nv.contact_list("0").unwrap().as_deref(), Some(&b"<contacts/>"[..]));
        assert_eq!(nv.cached_md5("0").unwrap().as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert!(nv.blf_items().unwrap().is_some());
        f.remove("contact_lists/0");
        assert_eq!(f.read("contact_lists/0"), None);
        assert!(f.list("ringtones").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choices_made_before_the_client_exists() {
        use opendeskphone_state::{Command, Reply};
        // "Use without a server": at once, and the client is told.
        let mut c = EarlyChoice::default();
        assert_eq!(c.take(&Command::SkipProvisioning), (Some(ProvisionPhase::Standalone), Ok(Reply::Done)));
        assert_eq!(c.replay(), [Command::SkipProvisioning]);
        // Setup ends with it (see `system_phase`).
        assert_eq!(system_phase(opendeskphone_state::Phase::Provisioning, &ProvisionPhase::Standalone, 0), opendeskphone_state::Phase::Running);
        // An address entered afterward is what counts.
        assert_eq!(c.take(&Command::StartProvisioning { server: Some(" 10.0.0.5:5060 ".into()) }), (Some(ProvisionPhase::Idle), Ok(Reply::Done)));
        assert_eq!(c.replay(), [Command::StartProvisioning { server: Some("10.0.0.5:5060".into()) }]);
        // Retry without an address keeps the one entered.
        assert_eq!(c.take(&Command::StartProvisioning { server: None }).1, Ok(Reply::Done));
        assert_eq!(c.replay(), [Command::StartProvisioning { server: Some("10.0.0.5:5060".into()) }]);
        // An address, then "Use without a server": the client gets both,
        // and stays out of the way until asked again.
        assert_eq!(c.take(&Command::SkipProvisioning), (Some(ProvisionPhase::Standalone), Ok(Reply::Done)));
        assert_eq!(c.replay(), [Command::StartProvisioning { server: Some("10.0.0.5:5060".into()) }, Command::SkipProvisioning]);
        // What needs the client says so.
        assert!(c.take(&Command::SelectUser { id: "100".into(), pin: String::new() }).1.is_err());
        assert!(c.take(&Command::ForgetProvisioning).1.is_err());
        assert!(EarlyChoice::default().replay().is_empty());
    }

    #[test]
    fn a_phone_starts_in_setup_without_a_server_and_accounts() {
        let account = r#"<config><accounts><account id="100" userid="100" authname="100" passwd="x"><host_primary server="pbx.test" port="5060" transport="udp"/></account></accounts></config>"#;
        let saved = br#"<config><setting id="config_server_url" value="sip:proxy@10.0.0.5:5060" /></config>"#;
        let http = br#"<config><setting id="config_server_url" value="http://pbx/cfg" /><setting id="config_server_type" value="curl" /></config>"#;
        // After a factory reset: no configurator.xml, no accounts.
        assert!(starts_in_setup(&config("<config/>"), None));
        assert!(starts_in_setup(&config("<config/>"), Some(b"<config>\n</config>\n")));
        // A saved DPMA server: the phone goes to it, whatever comes of it.
        assert!(!starts_in_setup(&config("<config/>"), Some(saved)));
        // Accounts of the phone's own.
        assert_eq!(config(account).accounts().len(), 1);
        assert!(!starts_in_setup(&config(account), None));
        // Stock's HTTP provisioning is not a server of ours.
        assert!(starts_in_setup(&config("<config/>"), Some(http)));
    }

    #[test]
    fn setup_is_for_a_phone_with_no_server_and_no_accounts() {
        use opendeskphone_state::Phase;
        let failed = ProvisionPhase::Failed { reason: "no answer".into(), code: None, detail: String::new() };
        // After a factory reset: no saved server, so the client waits for
        // one, and no accounts.  The server menu takes the display.
        assert_eq!(system_phase(Phase::Running, &ProvisionPhase::AwaitingServer, 0), Phase::Provisioning);
        // It keeps it before the client has started (no address yet),
        // through the choice of a server and of a user, and through a
        // failure.
        for p in [ProvisionPhase::AwaitingServer, ProvisionPhase::Handshaking, ProvisionPhase::AwaitingUser, ProvisionPhase::FetchingConfig, ProvisionPhase::Applying, failed.clone(), ProvisionPhase::Idle] {
            assert_eq!(system_phase(Phase::Provisioning, &p, 0), Phase::Provisioning, "{p:?}");
        }
        // Provisioned, or used without a server: the idle screen.
        assert_eq!(system_phase(Phase::Provisioning, &ProvisionPhase::Provisioned, 1), Phase::Running);
        assert_eq!(system_phase(Phase::Provisioning, &ProvisionPhase::Standalone, 0), Phase::Running);
        // A phone with a saved server goes straight to it and never waits
        // for one; whatever happens then, the idle screen stays.
        for p in [ProvisionPhase::Idle, ProvisionPhase::Handshaking, ProvisionPhase::AwaitingUser, ProvisionPhase::FetchingConfig, failed, ProvisionPhase::Provisioned] {
            assert_eq!(system_phase(Phase::Running, &p, 0), Phase::Running, "{p:?}");
        }
        // A phone with accounts of its own and no server is not held up.
        assert_eq!(system_phase(Phase::Running, &ProvisionPhase::AwaitingServer, 1), Phase::Running);
        // Start-up, recovery, and shutdown are not setup's to change.
        for now in [Phase::Starting, Phase::NetworkWait, Phase::Recovery, Phase::ShuttingDown] {
            assert_eq!(system_phase(now, &ProvisionPhase::AwaitingServer, 0), now);
        }
    }

    #[test]
    fn announcements() {
        let mut s = opendeskphone_net::mdns::Service {
            instance: "Digium Phone Config Server".into(),
            fullname: "Digium Phone Config Server._digiumproxy._udp.local".into(),
            host: "pbx.local".into(),
            port: 5060,
            addrs: vec!["10.0.0.5".parse().unwrap()],
            txt: [("serviceType".to_string(), "asterisk".to_string())].into(),
        };
        assert_eq!(announced(&s).unwrap().address, "10.0.0.5:5060");
        s.txt.insert("sipUrl".into(), "sip:proxy@10.0.0.5:5062;transport=tcp".into());
        let d = announced(&s).unwrap();
        assert_eq!((d.source, d.address.as_str()), (DiscoverySource::Mdns, "sip:proxy@10.0.0.5:5062;transport=tcp"));
        s.txt.clear();
        s.addrs.clear();
        assert_eq!(announced(&s).unwrap().address, "pbx.local:5060");
        s.host.clear();
        assert!(announced(&s).is_none());
    }
}
