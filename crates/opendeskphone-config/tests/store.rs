mod common;

use std::net::Ipv4Addr;

use common::{FETCHED, TempDir};
use opendeskphone_config::view::Transport;
use opendeskphone_config::{ConfigError, ConfigStore, Element, Layer, Model, Nvdata, SavedTime};

fn store(model: Model) -> (TempDir, ConfigStore) {
    let dir = TempDir::new("store");
    let nv = Nvdata::new(dir.path().join("nvdata"));
    nv.ensure_layout().unwrap();
    let store = ConfigStore::open(nv, model, dir.path().join("tmp")).unwrap();
    (dir, store)
}

#[test]
fn factory_layers_freeze_model_facts() {
    let expect = [
        (Model::D40, 2, "240", "120"),
        (Model::D45, 2, "240", "120"),
        (Model::D50, 4, "240", "120"),
        (Model::D60, 2, "480", "272"),
        (Model::D62, 2, "480", "272"),
        (Model::D65, 6, "480", "272"),
        (Model::D70, 6, "320", "160"),
        (Model::D80, 6, "800", "1280"),
    ];
    for (model, accounts, w, h) in expect {
        let (_d, mut s) = store(model);
        let c = s.config();
        assert_eq!(c.setting("phone_model"), Some(model.name()));
        assert_eq!(c.max_accounts(), accounts);
        assert_eq!(c.setting("width"), Some(w));
        assert_eq!(c.setting("height"), Some(h));
        for id in ["phone_model", "max_accounts", "width", "height"] {
            assert!(c.setting_frozen(id), "{model} {id}");
            assert!(matches!(s.set(id, "9"), Err(ConfigError::Frozen(_))));
        }
        assert_eq!(c.setting("enable_blf_on_unused_line_keys"), Some("1"));
        assert_eq!(c.softkeys("idle").len(), 6);
        assert_eq!(c.keymap_states().len(), if matches!(model, Model::D70 | Model::D80) { 18 } else { 19 }, "{model}");
        assert_eq!(c.enabled_codecs().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), [
            "PCMU", "PCMA", "G722", "G726-32"
        ]);
        // A server cannot raise the account limit either.
        s.apply_fetched(r#"<config><setting id="max_accounts" value="9"/></config>"#).unwrap();
        assert_eq!(s.config().max_accounts(), accounts);
    }
}

#[test]
fn set_goes_to_the_ui_layer_and_config_xml_stays_stock() {
    let (_d, mut s) = store(Model::D60);
    let g0 = s.config().generation();
    let c = s.set_many(&[("ringer_volume", "7")]).unwrap();
    assert!(c.generation() > g0);
    assert_eq!(c.setting("ringer_volume"), Some("7"));

    let nv = s.nvdata().clone();
    let ui = nv.read_string("ui-config.xml").unwrap().unwrap();
    assert_eq!(ui, "<config>\n    <setting id=\"ringer_volume\" value=\"7\" />\n</config>\n");
    // The retired private IDs are refused, so they cannot reach a stock file.
    assert!(matches!(s.set("open_ssh_enabled", "1"), Err(ConfigError::NotAStockSetting(_))));
    assert_eq!(nv.read_string("ui-config.xml").unwrap().unwrap(), ui);
    // The effective file stock reads carries nothing of ours.
    let effective = nv.read_string("config.xml").unwrap().unwrap();
    assert!(effective.contains("<setting id=\"ringer_volume\" value=\"7\" />"));
    assert!(!effective.contains("open_"));

    // Same value again: no new generation.
    let g1 = c.generation();
    assert_eq!(s.set("ringer_volume", "7").unwrap().generation(), g1);
    // Unset returns to the default.
    assert_eq!(s.unset("ringer_volume").unwrap().setting("ringer_volume"), Some("5"));
    // No temp files left behind.
    assert!(std::fs::read_dir(nv.root()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
}

#[test]
fn fetched_config_network_selection_and_typed_view() {
    let (_d, mut s) = store(Model::D60);
    s.apply_fetched(FETCHED).unwrap();

    // No address yet: only the /0 profile matches.
    let c = s.config();
    assert_eq!(c.selected_network(), Some("any"));
    let a = c.accounts();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].primary.as_ref().unwrap().server, "pbx.example.com");
    assert_eq!(c.logging().server, None);

    // On the office network the longer prefix wins.
    let c = s.set_ip(Some(Ipv4Addr::new(10, 10, 0, 87))).unwrap();
    assert_eq!(c.selected_network(), Some("office"));
    let a = c.accounts();
    // Three provisioned, but a D60 has two accounts.
    assert_eq!(a.len(), 2);
    assert_eq!((a[0].username.as_str(), a[0].authname.as_str(), a[0].line_label.as_str()), ("101", "101", "Alice"));
    let p = a[0].primary.as_ref().unwrap();
    assert_eq!((p.server.as_str(), p.port, p.transport, p.reregister, p.retry), ("10.10.0.2", 5061, Transport::Tls, 120, 20));
    assert_eq!(p.media_encryption.as_deref(), Some("sdes"));
    assert_eq!(a[0].alternate.as_ref().unwrap().server, "10.10.0.3");
    assert!(a[0].visual_voicemail && a[0].register && a[0].enabled);
    assert_eq!(a[1].primary.as_ref().unwrap().transport, Transport::Tcp);

    let log = c.logging();
    assert!(log.remote_enabled);
    assert_eq!((log.server.as_deref(), log.port), (Some("10.10.0.5"), 514));
    assert_eq!(c.time().time_zone, "America/Denver");
    assert_eq!(c.firmware_for_model().unwrap().version, "2_9_28");
    assert_eq!(c.firmwares().len(), 2);
    assert!(!c.access().ignore_provisioned_firmware);
    assert_eq!(c.contact_sources()[0].url, "http://10.10.0.2/contacts.xml");
    assert_eq!(c.blf_items_url(), Some("http://10.10.0.2/blf.xml"));
    let r = c.ringtones();
    assert_eq!(r.active, "Chime");
    assert_eq!(r.alerts[0].ring_type, "answer");
    assert_eq!(c.multicast_listeners()[0].addr, "239.1.1.1");
    assert_eq!(c.multicast_listeners()[0].interrupt_calls, 1);
    let groups = c.multicast_broadcasts();
    assert_eq!(groups.iter().map(|b| (b.id.as_str(), b.name.as_str(), b.codec.as_str())).collect::<Vec<_>>(), [
        ("b1", "Warehouse", "G722"),
        ("b2", "Office", "PCMU"),
    ]);
    let idle = c.softkeys("idle");
    assert_eq!(idle.len(), 2);
    assert_eq!(idle[1].args, ["voicemail"]);
    assert_eq!(c.softkeys("hold").len(), 4);
    assert_eq!(c.activity_indicator("hold").unwrap().color, "red");
    assert_eq!(c.locale_setting("tone_country"), Some("us"));
    assert_eq!(c.network().sip_dscp, 26);

    // A local value beats the provisioned one.
    assert_eq!(s.set("ringer_volume", "9").unwrap().setting("ringer_volume"), Some("9"));

    // Explicit selection overrides the address match.
    let c = s.select_network(Some("any")).unwrap();
    assert_eq!(c.selected_network(), Some("any"));
    assert_eq!(c.accounts().len(), 1);
    let c = s.select_network(None).unwrap();
    assert_eq!(c.selected_network(), Some("office"));

    // Local accounts replace the provisioned collection.
    let acct = Element::parse(
        r#"<account index="0" username="200" authname="200" number="200" password="x" passcode="x" display="200" line_label="200" register="1" status="1" type="sip"><host_primary server="192.0.2.1" port="5060"/></account>"#,
    )
    .unwrap();
    let c = s.set_local_accounts(std::slice::from_ref(&acct)).unwrap();
    assert_eq!(c.accounts().len(), 1);
    assert_eq!(c.accounts()[0].username, "200");
    assert!(matches!(
        s.set_local_accounts(&[acct.clone(), acct.clone(), acct.clone()]),
        Err(ConfigError::TooManyAccounts { max: 2, given: 3 })
    ));
    assert_eq!(s.clear_local_accounts().unwrap().accounts().len(), 2);
}

#[test]
fn lldp_layer_and_unparsable_layers() {
    let (_d, mut s) = store(Model::D40);
    let c = s.set_lldp(Some(&[("network_vlan_id", "20"), ("network_vlan_qos", "5")])).unwrap();
    assert_eq!(c.network().vlan_id, Some(20));
    // The stock merge renames this legacy ID.
    assert_eq!(c.network().rtp_qos, 5);
    assert_eq!(s.set_lldp(None).unwrap().network().vlan_id, None);

    // A truncated layer is skipped with a warning; the rest still merges.
    s.nvdata().write("fetched-config.xml", b"<config><setting id=\"ringer_volume\" value=\"1\"/>").unwrap();
    s.set("handset_volume", "2").unwrap();
    assert_eq!(s.warnings().len(), 1);
    assert_eq!(s.config().setting("ringer_volume"), Some("5"));
    assert_eq!(s.config().setting("handset_volume"), Some("2"));
    assert!(matches!(s.write_layer(Layer::G729Legacy, &Element::new("config")), Err(ConfigError::ReadOnlyLayer(_))));
}

/// The formats of the locale, as the stock phone application resolves
/// them: the active locale's (found by its id in any case), and for what
/// it lacks, or for a locale the configuration does not have, `en_US`'s.
#[test]
fn a_localization_format_is_the_active_locales_else_en_us() {
    let (_d, mut s) = store(Model::D60);
    let c = s
        .apply_fetched(concat!(
            "<config><setting id=\"locale\" value=\"de_de\"/>",
            "<locales><locale id=\"de_DE\"><setting id=\"time_short_format\" value=\"HH:mm\"/></locale></locales>",
            "</config>"
        ))
        .unwrap();
    assert_eq!(c.localization_setting("time_short_format"), Some("HH:mm"));
    assert_eq!(c.localization_setting("date_short_format"), Some("M/d/yy"));
    let c = s.apply_fetched("<config><setting id=\"locale\" value=\"fr_FR\"/></config>").unwrap();
    assert_eq!(c.localization_setting("time_short_format"), Some("h:mm a"));
}

/// A top-level setting of a format goes before the locale's.
#[test]
fn a_localization_format_set_at_the_top_level_goes_first() {
    let (_d, mut s) = store(Model::D60);
    let c = s.set("time_short_format", "HH.mm").unwrap();
    assert_eq!(c.localization_setting("time_short_format"), Some("HH.mm"));
}

#[test]
fn web_password() {
    let (_d, mut s) = store(Model::D60);
    // No digest file: the factory default, admin / 789, and nothing else.
    assert!(!s.nvdata().exists("etc/.passwd"));
    assert!(s.verify_web_password("admin", "789").unwrap());
    assert!(!s.verify_web_password("admin", "nope").unwrap());
    assert!(!s.verify_web_password("admin", "").unwrap());
    assert!(!s.verify_web_password("root", "789").unwrap());
    // The same hash stock ships in its default digest file.
    assert_eq!(s.web_ha1("admin").unwrap().as_deref(), Some("42c4089ad2eabaeb4d56e37078e21d62"));
    // Checking a login writes nothing.
    assert!(!s.nvdata().exists("etc/.passwd"));
    s.set_web_password("s3cret").unwrap();
    assert!(s.verify_web_password("admin", "s3cret").unwrap());
    assert!(!s.verify_web_password("admin", "789").unwrap());
    let file = s.nvdata().read_string("etc/.passwd").unwrap().unwrap();
    // MD5("admin:Digium Phone Admin Login:s3cret"), htdigest format, no newline.
    assert_eq!(file, format!("admin:Digium Phone Admin Login:{}", s.web_ha1("admin").unwrap().unwrap()));
    assert_eq!(file.len(), 63);
    assert_eq!(s.config().setting("login_password"), Some("s3cret"));
    // The RFC 2617 example credentials.
    assert_eq!(
        opendeskphone_config::nvdata::htdigest_ha1("Mufasa", "testrealm@host.com", "Circle Of Life"),
        "939e7578ed9e3c518a452acee763bce9"
    );
}

#[test]
fn default_web_password_without_file_or_setting() {
    // A configuration with no login_password at all (not even the base
    // default) still accepts exactly admin / 789.
    let (_d, mut s) = store(Model::D40);
    s.apply_fetched(r#"<config><setting id="login_password" conflict="replace"/></config>"#).unwrap();
    assert_eq!(s.config().setting("login_password"), None);
    assert!(!s.nvdata().exists("etc/.passwd"));
    assert!(s.verify_web_password("admin", "789").unwrap());
    assert!(!s.verify_web_password("admin", "").unwrap());
    assert!(!s.verify_web_password("admin", "7890").unwrap());
    // An empty digest file has no entry for the user either.
    s.nvdata().write("etc/.passwd", b"").unwrap();
    assert!(s.verify_web_password("admin", "789").unwrap());
    // First change: written in the stock htdigest form, and then only the
    // new password works.
    s.set_web_password("fresh").unwrap();
    assert_eq!(
        s.nvdata().read_string("etc/.passwd").unwrap().unwrap(),
        format!("admin:Digium Phone Admin Login:{}", opendeskphone_config::nvdata::htdigest_ha1("admin", "Digium Phone Admin Login", "fresh"))
    );
    assert!(s.verify_web_password("admin", "fresh").unwrap());
    assert!(!s.verify_web_password("admin", "789").unwrap());
}

#[test]
fn nvdata_files() {
    let dir = TempDir::new("nv");
    let nv = Nvdata::new(dir.path());
    nv.ensure_layout().unwrap();

    nv.write("etc/enet.cfg", b"ENETCFG_MAC=00:0f:d3:01:02:03\nENETCFG_DHCP=1\nENETCFG_IPADDR=\n").unwrap();
    assert_eq!(nv.mac().unwrap(), Some([0x00, 0x0f, 0xd3, 1, 2, 3]));
    let mut cfg = nv.enet_cfg().unwrap().unwrap();
    assert!(cfg.dhcp());
    cfg.set("ENETCFG_DHCP", "0");
    nv.write_enet_cfg(&cfg).unwrap();
    assert_eq!(nv.read_string("etc/enet.cfg").unwrap().unwrap(), "ENETCFG_MAC=00:0f:d3:01:02:03\nENETCFG_DHCP=0\nENETCFG_IPADDR=\n");

    let t = SavedTime::parse("Nov 12 16:14:43 2026\n").unwrap();
    assert_eq!(t.to_unix_utc(), 1_794_500_083);
    assert_eq!(SavedTime::from_unix_utc(1_794_500_083), t);
    assert_eq!(t.to_text(), "Nov 12 16:14:43 2026\n");
    nv.write_systime(&t).unwrap();
    assert_eq!(nv.systime().unwrap(), Some(t));

    nv.write_reboot_reason("dphone", "firmware update").unwrap();
    assert_eq!(nv.reboot_reason().unwrap().as_deref(), Some("dphone: firmware update"));

    // .tmp files are never read.
    std::fs::write(dir.path().join("config.xml.tmp"), b"<config>").unwrap();
    assert!(nv.read("config.xml.tmp").is_err());
    assert!(!nv.list("").unwrap().contains(&"config.xml.tmp".to_owned()));

    // A private file is readable by its owner alone from the start.
    nv.write_private("opendeskphone/web-tls-key.der", b"key").unwrap();
    assert_eq!(nv.read("opendeskphone/web-tls-key.der").unwrap().as_deref(), Some(&b"key"[..]));
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(nv.path("opendeskphone/web-tls-key.der")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    nv.write_authorized_keys(&["ssh-ed25519 AAAA dev".to_owned()]).unwrap();
    std::fs::write(nv.path("opendeskphone/authorized_keys"), "# c\n\nssh-ed25519 AAAA dev\n").unwrap();
    assert_eq!(nv.authorized_keys().unwrap(), ["ssh-ed25519 AAAA dev"]);

    nv.write_cached_md5("config", "0123456789abcdef0123456789abcdef").unwrap();
    assert_eq!(nv.cached_md5("config").unwrap().unwrap().len(), 32);
    assert_eq!(opendeskphone_config::nvdata::md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn factory_reset_follows_stock_whitelist() {
    let dir = TempDir::new("reset");
    let nv = Nvdata::new(dir.path());
    nv.ensure_layout().unwrap();
    for f in [
        "etc/enet.cfg", "etc/.passwd", "etc/passwd", "reboot_reason", "phoneservice_lockdown",
        "g729-legacy-support.xml", "g729-config.xml", "if-setup-debug", "config.xml", "ui-config.xml",
        "fetched-config.xml", "systime", "certs/a.pem", "appdata/x/y", "root/.ssh/authorized_keys",
        "opendeskphone/authorized_keys", "opendeskphone/ssh_host_ed25519_key", "opendeskphone/trial", "opendeskphone/settings.txtpb",
    ] {
        nv.write(f, b"x").unwrap();
    }
    assert!(!ConfigStore::run_pending_factory_reset(&nv, &[]).unwrap());
    nv.request_factory_reset().unwrap();
    assert!(ConfigStore::run_pending_factory_reset(&nv, &["opendeskphone/trial"]).unwrap());

    let mut left = Vec::new();
    fn walk(base: &std::path::Path, p: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                walk(base, &e.path(), out);
            } else {
                out.push(e.path().strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    walk(dir.path(), dir.path(), &mut left);
    left.sort();
    assert_eq!(left, [
        "etc/enet.cfg", "g729-config.xml", "g729-legacy-support.xml", "if-setup-debug",
        "opendeskphone/trial", "phoneservice_lockdown", "reboot_reason",
    ]);
    assert!(!nv.factory_reset_requested());
}

#[test]
fn contacts_document() {
    let groups = opendeskphone_config::contacts::parse_contacts(
        r#"<phonebooks><contacts id="example" group_name="Example">
            <contact id="1001" first_name="Reception" last_name="" contact_type="sip" subscribe_to="1001">
              <numbers><number dial="1001" label="Extension" primary="1"/><number dial="5551001" label="Cell"/></numbers>
            </contact></contacts></phonebooks>"#,
    )
    .unwrap();
    assert_eq!(groups.len(), 1);
    let c = &groups[0].contacts[0];
    assert_eq!((c.id.as_str(), c.first_name.as_str(), c.subscribe_to.as_deref()), ("1001", "Reception", Some("1001")));
    assert_eq!(c.primary_number().unwrap().dial, "1001");
}

#[test]
fn local_blf_items_file_takes_precedence() {
    let (_d, mut s) = store(Model::D60);
    s.apply_fetched(
        r#"<config><smart_blf><blf_items><blf_item location="main" index="0" contact_id="merged"/></blf_items></smart_blf></config>"#,
    )
    .unwrap();
    let ids = |items: Vec<opendeskphone_config::view::BlfItem>| items.into_iter().filter_map(|i| i.contact_id).collect::<Vec<_>>();
    assert_eq!(ids(s.blf_items().unwrap()), ["merged"]);
    assert_eq!(ids(ConfigStore::blf_items_from(s.nvdata(), &s.config())), ["merged"]);

    let local = Element::parse(
        r#"<config><smart_blf><blf_items><blf_item location="main" index="0" contact_id="local-1"/><blf_item location="side" index="3" contact_id="local-2"/></blf_items></smart_blf></config>"#,
    )
    .unwrap();
    s.nvdata().write_blf_items(&local).unwrap();
    assert_eq!(ids(s.blf_items().unwrap()), ["local-1", "local-2"]);
    assert_eq!(ids(ConfigStore::blf_items_from(s.nvdata(), &s.config())), ["local-1", "local-2"]);

    s.nvdata().remove("blf_items.xml").unwrap();
    assert_eq!(ids(s.blf_items().unwrap()), ["merged"]);
}

#[test]
fn provisioned_certificates_are_kept_in_stocks_layout() {
    let (_d, mut s) = store(Model::D60);
    let nv = s.nvdata().clone();
    let base = b"# base bundle\n-----BEGIN CERTIFICATE-----\nQkFTRQ==\n-----END CERTIFICATE-----\n";

    // Nothing provisioned and no base bundle: nothing is created.
    assert!(nv.update_trust(&s.config(), None).unwrap().is_empty());
    assert_eq!(nv.trust_bundle().unwrap(), None);

    // (Siblings are told apart by `id`, as everywhere in the merge.)
    let cfg = s
        .apply_fetched(
            r#"<config><certs>
                 <cert id="a" label="pbx-ca" md5="0123456789abcdef0123456789abcdef">Q0E=</cert>
                 <cert id="b" label="second">Q0Ey</cert>
                 <clientcert label="phone.crt" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa">Q0VSVA==</clientcert>
                 <clientkey id="a" label="phone.key" type="rsa">S0VZ</clientkey>
                 <clientkey id="b" label="plain.key">S0VZMg==</clientkey>
                 <clientkey id="c" label="../escape">eA==</clientkey>
               </certs></config>"#,
        )
        .unwrap();
    let certs = cfg.provisioned_certs();
    assert_eq!((certs.trusted.len(), certs.client_certs.len(), certs.client_keys.len()), (2, 1, 3));

    let change = nv.update_trust(&cfg, Some(base)).unwrap();
    let mut written = change.written.clone();
    written.sort();
    assert_eq!(written, [
        "certs/certs.crt",
        "clientcerts/phone.crt",
        "clientcerts/phone.key",
        "clientcerts/plain.key",
        "md5s/pbx-ca",
        "md5s/phone.crt",
    ]);
    assert_eq!(change.skipped.len(), 1, "{:?}", change.skipped);

    // The bundle: the base, then each certificate as stock wraps it.
    let mut expect = base.to_vec();
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nQ0E=\n-----END CERTIFICATE-----\n");
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nQ0Ey\n-----END CERTIFICATE-----\n");
    assert_eq!(nv.trust_bundle().unwrap().unwrap(), expect);
    assert_eq!(opendeskphone_config::trust::trust_bundle(&cfg, Some(base)), expect);
    assert_eq!(
        nv.client_cert("phone.crt").unwrap().unwrap(),
        b"\n-----BEGIN CERTIFICATE-----\nQ0VSVA==\n-----END CERTIFICATE-----\n"
    );
    assert_eq!(
        nv.client_cert("phone.key").unwrap().unwrap(),
        b"\n-----BEGIN RSA PRIVATE KEY-----\nS0VZ\n-----END RSA PRIVATE KEY-----\n"
    );
    assert_eq!(
        nv.client_cert("plain.key").unwrap().unwrap(),
        b"\n-----BEGIN PRIVATE KEY-----\nS0VZMg==\n-----END PRIVATE KEY-----\n"
    );
    assert_eq!(nv.cached_md5("pbx-ca").unwrap().as_deref(), Some("0123456789abcdef0123456789abcdef"));
    assert!(!nv.path("escape").exists() && !nv.path("clientcerts/../escape").exists());

    // Unchanged configuration: nothing is written again.
    assert!(nv.update_trust(&cfg, Some(base)).unwrap().is_empty());

    // A certificate withdrawn by the server leaves the bundle.
    let cfg = s.apply_fetched(r#"<config><certs><cert id="b" label="second">Q0Ey</cert></certs></config>"#).unwrap();
    assert_eq!(nv.update_trust(&cfg, Some(base)).unwrap().written, ["certs/certs.crt"]);
    let text = String::from_utf8(nv.trust_bundle().unwrap().unwrap()).unwrap();
    assert!(text.contains("Q0Ey") && !text.contains("Q0E=\n"));
}

const BASE: &[u8] = b"# base bundle\n\nFirst CA\n-----BEGIN CERTIFICATE-----\nQkFTRTE=\n-----END CERTIFICATE-----\n\nSecond CA\n-----BEGIN CERTIFICATE-----\nQkFTRTI=\n-----END CERTIFICATE-----\n";

fn blocks(bundle: &[u8], body: &str) -> usize {
    String::from_utf8_lossy(bundle).matches(&format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n")).count()
}

/// The roots TLS clients get, with what is on disk.
fn roots(nv: &opendeskphone_config::Nvdata, cfg: &opendeskphone_config::Config, base: Option<&[u8]>) -> Vec<u8> {
    opendeskphone_config::trust::roots(cfg, base, nv.trust_bundle().unwrap().as_deref())
}

#[test]
fn trust_bundle_is_the_base_alone_without_provisioned_certificates() {
    // Also the state after a factory reset: no `certs/certs.crt`.
    let (_d, s) = store(Model::D60);
    let nv = s.nvdata().clone();
    // TLS clients already trust the base before anything is written.
    assert_eq!(roots(&nv, &s.config(), Some(BASE)), BASE);

    assert_eq!(nv.update_trust(&s.config(), Some(BASE)).unwrap().written, ["certs/certs.crt"]);
    assert_eq!(nv.trust_bundle().unwrap().unwrap(), BASE);
    assert_eq!(roots(&nv, &s.config(), Some(BASE)), BASE);
    assert!(nv.update_trust(&s.config(), Some(BASE)).unwrap().is_empty());
}

#[test]
fn trust_bundle_is_the_base_followed_by_provisioned_certificates() {
    let (_d, mut s) = store(Model::D60);
    let nv = s.nvdata().clone();
    let cfg = s
        .apply_fetched(r#"<config><certs><cert id="a" label="pbx-ca">UEJY</cert><cert id="b" label="other">T1RIRVI=</cert></certs></config>"#)
        .unwrap();
    nv.update_trust(&cfg, Some(BASE)).unwrap();
    let mut expect = BASE.to_vec();
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nUEJY\n-----END CERTIFICATE-----\n");
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nT1RIRVI=\n-----END CERTIFICATE-----\n");
    assert_eq!(nv.trust_bundle().unwrap().unwrap(), expect);
    assert_eq!(roots(&nv, &cfg, Some(BASE)), expect);

    // A factory reset removes `certs/` and the fetched configuration: the
    // next start trusts the base alone again.
    std::fs::remove_dir_all(nv.path("certs")).unwrap();
    let cfg = s.apply_fetched("<config/>").unwrap();
    assert_eq!(roots(&nv, &cfg, Some(BASE)), BASE);
    assert_eq!(nv.update_trust(&cfg, Some(BASE)).unwrap().written, ["certs/certs.crt"]);
    assert_eq!(nv.trust_bundle().unwrap().unwrap(), BASE);
}

#[test]
fn provisioned_certificate_that_the_base_has_is_listed_twice() {
    // Stock appends every `<cert>` without comparing it with the base or
    // with the others.
    let (_d, mut s) = store(Model::D60);
    let nv = s.nvdata().clone();
    let cfg = s
        .apply_fetched(r#"<config><certs><cert id="a" label="first">QkFTRTE=</cert><cert id="b" label="again">QkFTRTE=</cert></certs></config>"#)
        .unwrap();
    nv.update_trust(&cfg, Some(BASE)).unwrap();
    let bundle = nv.trust_bundle().unwrap().unwrap();
    assert!(bundle.starts_with(BASE));
    assert_eq!((blocks(&bundle, "QkFTRTE="), blocks(&bundle, "QkFTRTI=")), (3, 1));

    // The entry `a` with other content replaces what `a` held; the base's
    // own copy stays.
    let cfg = s.apply_fetched(r#"<config><certs><cert id="a" label="first">TkVX</cert></certs></config>"#).unwrap();
    assert_eq!(nv.update_trust(&cfg, Some(BASE)).unwrap().written, ["certs/certs.crt"]);
    let bundle = nv.trust_bundle().unwrap().unwrap();
    assert_eq!((blocks(&bundle, "QkFTRTE="), blocks(&bundle, "TkVX")), (1, 1));
    assert_eq!(roots(&nv, &cfg, Some(BASE)), bundle);
}

#[test]
fn trust_bundle_written_by_stock_is_rebuilt_from_our_base() {
    let (_d, mut s) = store(Model::D60);
    let nv = s.nvdata().clone();
    // Stock's file: its own base bundle and a certificate provisioned then.
    let stock = b"# stock's bundle\n-----BEGIN CERTIFICATE-----\nU1RPQ0s=\n-----END CERTIFICATE-----\n\n-----BEGIN CERTIFICATE-----\nT0xE\n-----END CERTIFICATE-----\n";
    nv.write("certs/certs.crt", stock).unwrap();
    nv.write("certs/phonekey.pem", b"key").unwrap();
    let cfg = s.apply_fetched(r#"<config><certs><cert id="a" label="pbx-ca">UEJY</cert></certs></config>"#).unwrap();

    // Before the file is rewritten, clients already get the new roots.
    let mut expect = BASE.to_vec();
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nUEJY\n-----END CERTIFICATE-----\n");
    assert_eq!(roots(&nv, &cfg, Some(BASE)), expect);

    assert_eq!(nv.update_trust(&cfg, Some(BASE)).unwrap().written, ["certs/certs.crt"]);
    assert_eq!(nv.trust_bundle().unwrap().unwrap(), expect);
    // Nothing else in `certs/` is touched.
    assert_eq!(nv.list("certs").unwrap(), ["certs.crt", "phonekey.pem"]);
    assert_eq!(nv.read("certs/phonekey.pem").unwrap().unwrap(), b"key");
}

#[test]
fn without_a_base_bundle_the_file_on_disk_is_kept_and_extended() {
    // A run outside an image (no `/usr/share/ssl/certs/cacert.pem`).
    let (_d, mut s) = store(Model::D60);
    let nv = s.nvdata().clone();
    let cfg = s.apply_fetched(r#"<config><certs><cert id="a" label="pbx-ca">UEJY</cert></certs></config>"#).unwrap();
    assert_eq!(roots(&nv, &cfg, None), b"\n-----BEGIN CERTIFICATE-----\nUEJY\n-----END CERTIFICATE-----\n");
    nv.write("certs/certs.crt", BASE).unwrap();
    let mut expect = BASE.to_vec();
    expect.extend_from_slice(b"\n-----BEGIN CERTIFICATE-----\nUEJY\n-----END CERTIFICATE-----\n");
    assert_eq!(roots(&nv, &cfg, None), expect);
}

#[test]
fn user_ringtones_are_the_sln16_files() {
    let (_d, s) = store(Model::D60);
    let nv = s.nvdata();
    assert!(nv.user_ringtones().unwrap().is_empty());
    nv.write("ringtones/Mine.sln16", &[1, 0, 2, 0]).unwrap();
    nv.write("ringtones/notes.txt", b"x").unwrap();
    assert_eq!(nv.user_ringtones().unwrap(), ["Mine"]);
    assert_eq!(nv.user_ringtone("Mine").unwrap().unwrap(), [1, 0, 2, 0]);
    assert_eq!(nv.user_ringtone("Other").unwrap(), None);
    assert!(nv.user_ringtone("../etc/enet.cfg").is_err());
}
