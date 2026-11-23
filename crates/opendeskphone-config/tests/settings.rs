//! `/nvdata/opendeskphone/settings.txtpb` through the configuration store.

mod common;

use common::TempDir;
use opendeskphone_config::{ConfigStore, OpenDeskPhoneSettings, IdleSoftkey, Model, Nvdata, Resolved, SettingSource};

const FILE: &str = "opendeskphone/settings.txtpb";

fn store() -> (TempDir, Nvdata, ConfigStore) {
    let dir = TempDir::new("settings");
    let nv = Nvdata::new(dir.path().join("nvdata"));
    nv.ensure_layout().unwrap();
    let store = ConfigStore::open(nv.clone(), Model::D60, dir.path().join("tmp")).unwrap();
    (dir, nv, store)
}

fn reopen(dir: &TempDir, nv: &Nvdata) -> ConfigStore {
    ConfigStore::open(nv.clone(), Model::D60, dir.path().join("tmp")).unwrap()
}

#[test]
fn defaults_with_no_file() {
    let (_d, nv, s) = store();
    assert!(!nv.exists(FILE));
    assert_eq!(*s.opendeskphone_settings(), OpenDeskPhoneSettings::default());
    let c = s.config();
    let off = Resolved { value: false, source: SettingSource::Default, locked: false };
    let on = Resolved { value: true, ..off };
    assert_eq!((c.ssh_enabled(), c.ignore_provisioned_firmware()), (off, off));
    // UART A is the EHS headset adapter's unless the console is asked for.
    assert_eq!((c.serial_console(), c.metrics_enabled()), (off, on));
    let a = c.access();
    // SSH off; web configuration on; the server's firmware is followed, as
    // on stock.
    assert!(!a.ssh_enabled && a.web_ui_enabled && !a.ignore_provisioned_firmware);
    // Opening the store does not create the file.
    assert!(!nv.exists(FILE));
}

#[test]
fn update_round_trip_presence_and_generation() {
    let (dir, nv, mut s) = store();
    let g0 = s.config().generation();
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(false)).unwrap();
    assert_eq!(c.generation(), g0 + 1);
    assert!(!c.access().ssh_enabled);
    assert!(!c.access().ignore_provisioned_firmware);
    // Only the field that was set is written.
    let text = nv.read_string(FILE).unwrap().unwrap();
    let fields: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(fields, ["ssh_enabled: false"]);
    // No temp file left behind (read from the directory itself, since
    // `Nvdata::list` leaves `.tmp` files out).
    let names = std::fs::read_dir(nv.path("opendeskphone")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned());
    assert_eq!(names.filter(|n| n.starts_with("settings")).collect::<Vec<_>>(), ["settings.txtpb"]);

    // A change that changes nothing: no write, no new generation.  The
    // file's time is set back first, so that any write would show.
    let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(nv.path(FILE)).unwrap().set_modified(long_ago).unwrap();
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(false)).unwrap();
    assert_eq!(c.generation(), g0 + 1);
    assert_eq!(std::fs::metadata(nv.path(FILE)).unwrap().modified().unwrap(), long_ago);

    // Explicit false differs from unset, though both read as disabled.
    assert_eq!(c.opendeskphone_settings().ssh_enabled, Some(false));
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(true)).unwrap();
    assert_eq!(c.generation(), g0 + 2);
    assert_eq!(c.opendeskphone_settings().ssh_enabled, Some(true));
    assert!(c.access().ssh_enabled);
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("ssh_enabled: true"));
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = None).unwrap();
    assert_eq!(c.generation(), g0 + 3);
    assert_eq!(c.opendeskphone_settings().ssh_enabled, None);
    assert!(!c.access().ssh_enabled);
    assert!(!nv.read_string(FILE).unwrap().unwrap().contains("ssh_enabled"));

    // Survives a restart.
    s.update_opendeskphone_settings(|d| {
        d.ssh_enabled = Some(true);
        d.ignore_provisioned_firmware = Some(true);
    })
    .unwrap();
    let s2 = reopen(&dir, &nv);
    let a = s2.config().access();
    assert!(a.ssh_enabled && a.ignore_provisioned_firmware);

    // The stock-facing files are untouched by all this.
    assert!(!nv.read_string("config.xml").unwrap().unwrap().contains("ssh"));
    assert!(!nv.exists("ui-config.xml"));

    // A stock setting change does not disturb the settings file either.
    let text = nv.read_string(FILE).unwrap().unwrap();
    s.set("ringer_volume", "2").unwrap();
    assert_eq!(nv.read_string(FILE).unwrap().unwrap(), text);
}

#[test]
fn file_edited_by_hand_is_picked_up_on_reload() {
    let (_d, nv, mut s) = store();
    let g0 = s.config().generation();
    nv.write(FILE, b"# by hand\nssh_enabled: false\n").unwrap();
    let c = s.reload().unwrap();
    assert_eq!(c.generation(), g0 + 1);
    assert!(!c.access().ssh_enabled);
}

#[test]
fn unknown_fields_survive_an_update() {
    let (dir, nv, mut s) = store();
    nv.write(FILE, b"ssh_enabled: true\nring_pattern: \"double\"  # from a newer firmware\nled { brightness: 3 }\n")
        .unwrap();
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(false)).unwrap();
    assert!(!c.access().ssh_enabled);
    let text = nv.read_string(FILE).unwrap().unwrap();
    assert!(text.contains("ssh_enabled: false\n"));
    assert!(text.contains("ring_pattern: \"double\"  # from a newer firmware\n"));
    assert!(text.contains("led { brightness: 3 }\n"));
    // And again through a restart and a second update.
    let mut s2 = reopen(&dir, &nv);
    s2.update_opendeskphone_settings(|d| d.ignore_provisioned_firmware = Some(true)).unwrap();
    let text = nv.read_string(FILE).unwrap().unwrap();
    assert!(text.contains("ring_pattern: \"double\"") && text.contains("led { brightness: 3 }"));
}

#[test]
fn malformed_file_is_set_aside_and_defaults_apply() {
    let dir = TempDir::new("settings-bad");
    let nv = Nvdata::new(dir.path().join("nvdata"));
    nv.ensure_layout().unwrap();
    let bad = b"ssh_enabled: true\nignore_provisioned_firmware: {{{ not text format\n";
    nv.write(FILE, bad).unwrap();
    let mut s = reopen(&dir, &nv);
    // Configuration still comes up, on defaults.
    let c = s.config();
    assert!(!c.access().ssh_enabled && !c.access().ignore_provisioned_firmware);
    assert_eq!(c.setting("phone_model"), Some("D60"));
    // The bad file is kept for inspection, not deleted and not left in place.
    assert!(!nv.exists(FILE));
    assert_eq!(nv.read("opendeskphone/settings.txtpb.bad").unwrap().unwrap(), bad);
    // And the store keeps working.
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(true)).unwrap();
    assert!(c.access().ssh_enabled);
    assert!(nv.exists(FILE));
}

#[test]
fn factory_reset_returns_the_settings_to_defaults() {
    let (dir, nv, mut s) = store();
    s.update_opendeskphone_settings(|d| {
        d.ssh_enabled = Some(true);
        d.ignore_provisioned_firmware = Some(true);
    })
    .unwrap();
    nv.write("opendeskphone/settings.txtpb.bad", b"x").unwrap();
    nv.request_factory_reset().unwrap();
    assert!(ConfigStore::run_pending_factory_reset(&nv, &[]).unwrap());
    assert!(!nv.exists(FILE));
    assert!(!nv.exists("opendeskphone/settings.txtpb.bad"));
    let s = reopen(&dir, &nv);
    assert_eq!(*s.opendeskphone_settings(), OpenDeskPhoneSettings::default());
    assert!(!s.config().access().ssh_enabled && !s.config().access().ignore_provisioned_firmware);
}

// ---- provisioning from DPMA ------------------------------------------------

const KEY_A: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa alice@laptop";
const KEY_B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb bob@desk";
const KEY_LOCAL: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICcccccccccccccccccccccccccccccccccccccccccc local";

fn provisioned(extra: &str) -> String {
    format!("<config><setting id=\"ringer_volume\" value=\"3\"/>{extra}</config>")
}

#[test]
fn provisioned_settings_precedence_and_source() {
    use SettingSource::*;
    let (_d, nv, mut s) = store();
    // Provisioned, nothing local: the provisioned value applies.
    let c = s
        .apply_fetched(&provisioned(
            r#"<setting id="opendeskphone_ssh_enabled" value="0"/><setting id="opendeskphone_ignore_provisioned_firmware" value="1"/>"#,
        ))
        .unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: false, source: Provisioned, locked: false });
    assert_eq!(c.ignore_provisioned_firmware(), Resolved { value: true, source: Provisioned, locked: false });
    // The retired HTTPS setting is not recognized: it is inert, like any
    // unknown setting.
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_ssh_enabled" value="0"/><setting id="opendeskphone_ignore_provisioned_firmware" value="1"/><setting id="opendeskphone_https_enabled" value="0"/>"#)).unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: false, source: Provisioned, locked: false });
    assert!(!c.access().ssh_enabled && c.access().ignore_provisioned_firmware);
    // Provisioning never writes the local file.
    assert!(!nv.exists(FILE));

    // A local value beats an ordinary provisioned one, as ui-config.xml
    // beats fetched-config.xml on stock.
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(true)).unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: true, source: Local, locked: false });
    // Unsetting it locally shows the provisioned value again.
    let c = s.update_opendeskphone_settings(|d| d.ssh_enabled = None).unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: false, source: Provisioned, locked: false });

    // conflict="freeze" from the server wins over the local value.
    s.update_opendeskphone_settings(|d| d.ssh_enabled = Some(true)).unwrap();
    let c = s
        .apply_fetched(&provisioned(r#"<setting id="opendeskphone_ssh_enabled" value="0" conflict="freeze"/>"#))
        .unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: false, source: Provisioned, locked: true });
    // The local value is kept, and applies again once the server lets go.
    let c = s.apply_fetched(&provisioned("")).unwrap();
    assert_eq!(c.ssh_enabled(), Resolved { value: true, source: Local, locked: false });
    assert_eq!(c.ignore_provisioned_firmware(), Resolved { value: false, source: Default, locked: false });

    // `metrics_enabled` follows the same rules; its default is on.
    assert_eq!(c.metrics_enabled(), Resolved { value: true, source: Default, locked: false });
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_metrics_enabled" value="0"/>"#)).unwrap();
    assert_eq!(c.metrics_enabled(), Resolved { value: false, source: Provisioned, locked: false });
    let c = s.update_opendeskphone_settings(|d| d.metrics_enabled = Some(true)).unwrap();
    assert_eq!(c.metrics_enabled(), Resolved { value: true, source: Local, locked: false });
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("metrics_enabled: true"));
    let c = s
        .apply_fetched(&provisioned(r#"<setting id="opendeskphone_metrics_enabled" value="0" conflict="freeze"/>"#))
        .unwrap();
    assert_eq!(c.metrics_enabled(), Resolved { value: false, source: Provisioned, locked: true });
    let c = s.apply_fetched(&provisioned("")).unwrap();
    assert_eq!(c.metrics_enabled(), Resolved { value: true, source: Local, locked: false });
    let c = s.update_opendeskphone_settings(|d| d.metrics_enabled = None).unwrap();
    assert_eq!(c.metrics_enabled(), Resolved { value: true, source: Default, locked: false });

    // The provisioned IDs cannot be written through the generic path into
    // ui-config.xml.
    assert!(s.set("opendeskphone_metrics_enabled", "0").is_err());
    assert!(s.set("opendeskphone_ssh_enabled", "1").is_err());
    assert!(!nv.exists("ui-config.xml"));
}

#[test]
fn provisioned_static_address_and_the_stock_network_settings() {
    use std::net::Ipv4Addr;
    use SettingSource::*;
    let (_d, _nv, mut s) = store();
    let address = |c: &opendeskphone_config::Config| c.static_ipv4().map(|a| (a.address, a.prefix_len, a.source, a.locked));
    // Nothing set, and what every phone that went through stock's setup
    // has in ui-config.xml: DHCP.
    assert_eq!(s.config().static_ipv4(), None);
    let c = s.set_many(&[("network_enable_dhcp", "1"), ("network_static_ip_address", ""), ("network_subnet_mask", "")]).unwrap();
    assert_eq!(c.static_ipv4(), None);

    // The server's address applies over that, with its gateway and DNS
    // servers; what is not an address is left out.
    let c = s
        .apply_fetched(&provisioned(
            r#"<setting id="opendeskphone_static_address" value="198.18.14.120/23"/><setting id="opendeskphone_static_gateway" value="198.18.14.1"/><setting id="opendeskphone_static_dns" value="198.18.14.1, 198.18.14.2,dns,198.18.14.1"/>"#,
        ))
        .unwrap();
    let a = c.static_ipv4().unwrap();
    assert_eq!((a.address, a.prefix_len, a.source, a.locked), (Ipv4Addr::new(198, 18, 14, 120), 23, Provisioned, false));
    assert_eq!(a.gateway, Some(Ipv4Addr::new(198, 18, 14, 1)));
    assert_eq!(a.dns, [Ipv4Addr::new(198, 18, 14, 1), Ipv4Addr::new(198, 18, 14, 2)]);
    // Stock's own view of the network settings is untouched.
    assert!(c.network().dhcp);

    // A static address set on the phone beats an ordinary provisioned one...
    let c = s
        .set_many(&[("network_enable_dhcp", "0"), ("network_static_ip_address", "192.168.7.9"), ("network_subnet_mask", "255.255.255.0"), ("network_default_gateway", "192.168.7.1")])
        .unwrap();
    let a = c.static_ipv4().unwrap();
    assert_eq!((a.address, a.prefix_len, a.gateway, a.source), (Ipv4Addr::new(192, 168, 7, 9), 24, Some(Ipv4Addr::new(192, 168, 7, 1)), Local));
    // ...but not a frozen one.
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_static_address" value="198.18.14.120/23" conflict="freeze"/>"#)).unwrap();
    assert_eq!(address(&c), Some((Ipv4Addr::new(198, 18, 14, 120), 23, Provisioned, true)));
    // The server lets go: the phone's own address, then DHCP again.
    let c = s.apply_fetched(&provisioned("")).unwrap();
    assert_eq!(address(&c), Some((Ipv4Addr::new(192, 168, 7, 9), 24, Local, false)));
    let c = s.set("network_enable_dhcp", "1").unwrap();
    assert_eq!(c.static_ipv4(), None);

    // An address that is not one for a host is ignored whole.
    for bad in ["198.18.14.120", "198.18.14.120/32", "198.18.14.0/23", "198.18.15.255/23", "224.0.0.9/4", "0.0.0.1/8", "198.18.14.120/255.255.254.0", "ten/8", ""] {
        let c = s
            .apply_fetched(&provisioned(&format!(r#"<setting id="opendeskphone_static_address" value="{bad}"/><setting id="opendeskphone_static_gateway" value="198.18.14.1"/>"#)))
            .unwrap();
        assert_eq!(c.static_ipv4(), None, "{bad:?}");
    }
    // The IDs cannot be written into ui-config.xml.
    assert!(s.set("opendeskphone_static_address", "10.0.0.2/8").is_err());
}

#[test]
fn provisioned_ssh_keys_combine_with_local_ones() {
    let (dir, nv, mut s) = store();
    let mirror = "opendeskphone/provisioned_authorized_keys";
    nv.write_authorized_keys(&[KEY_LOCAL.to_owned()]).unwrap();
    assert!(!nv.exists(mirror));

    let keys = |list: &[&str]| {
        let items: String =
            list.iter().enumerate().map(|(i, k)| format!("<key id=\"{}\" value=\"{k}\"/>", i + 1)).collect();
        provisioned(&format!("<opendeskphone_ssh_authorized_keys>{items}</opendeskphone_ssh_authorized_keys>"))
    };
    // Two keys, a duplicate, and two entries that are not plain key lines.
    let c = s
        .apply_fetched(&keys(&[KEY_A, KEY_B, KEY_A, "command=&quot;sh&quot; ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA", "junk"]))
        .unwrap();
    assert_eq!(c.provisioned_ssh_keys(), [KEY_A, KEY_B]);
    assert_eq!(s.ssh_authorized_keys().unwrap(), [KEY_LOCAL, KEY_A, KEY_B]);
    assert_eq!(nv.read_string(mirror).unwrap().unwrap(), format!("{KEY_A}\n{KEY_B}\n"));
    // The local file is never touched by provisioning.
    assert_eq!(nv.authorized_keys().unwrap(), [KEY_LOCAL]);

    // The server drops one key: it is gone, with nothing stale left behind.
    let c = s.apply_fetched(&keys(&[KEY_B])).unwrap();
    assert_eq!(c.provisioned_ssh_keys(), [KEY_B]);
    assert_eq!(nv.read_string(mirror).unwrap().unwrap(), format!("{KEY_B}\n"));
    // Still right after a restart.
    let s2 = reopen(&dir, &nv);
    assert_eq!(s2.ssh_authorized_keys().unwrap(), [KEY_LOCAL, KEY_B]);

    // The server stops sending keys: only the local one remains.
    s.apply_fetched(&provisioned("")).unwrap();
    assert!(!nv.exists(mirror));
    assert_eq!(s.ssh_authorized_keys().unwrap(), [KEY_LOCAL]);

    // A stale mirror (for example, after fetched-config.xml was removed
    // while the firmware was not running) is cleaned up at the next start.
    nv.write(mirror, format!("{KEY_A}\n").as_bytes()).unwrap();
    let s3 = reopen(&dir, &nv);
    assert!(!nv.exists(mirror));
    assert_eq!(s3.ssh_authorized_keys().unwrap(), [KEY_LOCAL]);
}

/// The default softkeys end with the theme selector on the models that
/// have themes (the color ones), and with Forward on the others.
#[test]
fn default_idle_softkeys_by_model() {
    use opendeskphone_config::IdleFunction::*;
    let common = [CallLog, Contacts, Status, Menu, Apps, Dnd, Forward].map(IdleSoftkey::Function);
    for model in Model::ALL {
        let dir = TempDir::new("softkeys");
        let nv = Nvdata::new(dir.path().join("nvdata"));
        nv.ensure_layout().unwrap();
        let mut s = ConfigStore::open(nv, model, dir.path().join("tmp")).unwrap();
        let keys = s.config().idle_softkeys().keys;
        let color = matches!(model, Model::D60 | Model::D62 | Model::D65 | Model::D80);
        assert_eq!(keys[..7], common, "{model:?}");
        if color {
            assert_eq!(keys[7..], [IdleSoftkey::Function(Theme)], "{model:?}: Theme after Forward");
        } else {
            assert_eq!(keys.len(), 7, "{model:?}: no theme selector");
        }
        // A list that names it is the list on every model (a gray
        // model's screen leaves the softkey out).
        s.apply_fetched(r#"<config><setting id="opendeskphone_idle_softkeys" value="menu,theme"/></config>"#).unwrap();
        assert_eq!(s.config().idle_softkeys().keys, [IdleSoftkey::Function(Menu), IdleSoftkey::Function(Theme)]);
        assert_eq!(Theme.on_model(model), color);
    }
}

#[test]
fn idle_softkeys_sources() {
    let (_d, nv, mut s) = store();
    use opendeskphone_config::IdleFunction::*;
    let f = IdleSoftkey::Function;
    // Nothing set: the default list, which on this color model ends with
    // the theme selector.
    let default = [f(CallLog), f(Contacts), f(Status), f(Menu), f(Apps), f(Dnd), f(Forward), f(Theme)];
    let c = s.config();
    assert_eq!(c.idle_softkeys().keys, default);
    assert_eq!((c.idle_softkeys().source, c.idle_softkeys().locked), (SettingSource::Default, false));

    // The `idle` keymap state a DPMA server sends to every phone is not a
    // list that was set: the default stays.
    let keymap = r#"<keymap><state id="idle" conflict="freeze"><softkeys>
        <key id="0" action="missed_or_call_log"/>
        <key id="1" action="show_application" label1="_PARKED_CALLS"><arg>parking</arg></key>
        <key id="2" action="info"/>
        <key id="3" action="menu"/>
    </softkeys></state></keymap>"#;
    s.apply_fetched(&format!("<config>{keymap}</config>")).unwrap();
    let c = s.config();
    assert_eq!(c.softkeys("idle").len(), 4);
    assert_eq!(c.idle_softkeys().keys, default);
    assert_eq!(c.idle_softkeys().source, SettingSource::Default);

    // A provisioned list is shown, and a local list before that.
    s.apply_fetched(&format!(r#"<config>{keymap}<setting id="opendeskphone_idle_softkeys" value="menu, apps"/></config>"#)).unwrap();
    assert_eq!(s.config().idle_softkeys().keys, [f(Menu), f(Apps)]);
    assert_eq!(s.config().idle_softkeys().source, SettingSource::Provisioned);
    let local = [f(Dnd), IdleSoftkey::App("queues".into())];
    let c = s.update_opendeskphone_settings(|d| d.set_idle_softkeys(&local)).unwrap();
    assert_eq!(c.idle_softkeys().keys, local);
    assert_eq!((c.idle_softkeys().source, c.idle_softkeys().locked), (SettingSource::Local, false));
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("idle_softkeys {\n  function: FUNCTION_DND\n}\nidle_softkeys {\n  app {\n    name: \"queues\"\n  }\n}\n"));

    // A provisioned list that does not parse counts as not set, and so
    // does a local list with nothing in it.
    s.apply_fetched(r#"<config><setting id="opendeskphone_idle_softkeys" value="menu,lunch"/></config>"#).unwrap();
    assert_eq!(s.config().idle_softkeys().keys, local);
    s.apply_fetched(r#"<config><setting id="opendeskphone_idle_softkeys" value="menu, apps"/></config>"#).unwrap();
    let c = s.update_opendeskphone_settings(|d| d.set_idle_softkeys(&[])).unwrap();
    assert_eq!(c.idle_softkeys().keys, [f(Menu), f(Apps)]);

    // Frozen by the server: the local list has no effect.
    s.update_opendeskphone_settings(|d| d.set_idle_softkeys(&[f(Dnd)])).unwrap();
    s.apply_fetched(r#"<config><setting id="opendeskphone_idle_softkeys" value="call_log" conflict="freeze"/></config>"#).unwrap();
    let c = s.config();
    assert_eq!(c.idle_softkeys().keys, [f(CallLog)]);
    assert_eq!((c.idle_softkeys().source, c.idle_softkeys().locked), (SettingSource::Provisioned, true));

    // Neither list set any more: the default again.
    s.apply_fetched("<config/>").unwrap();
    let c = s.update_opendeskphone_settings(|d| d.set_idle_softkeys(&[])).unwrap();
    assert_eq!(c.idle_softkeys().keys, default);
}

#[test]
fn disabled_apps_sources() {
    use SettingSource::*;
    let (_d, nv, mut s) = store();
    let apps = |a: &[&str]| a.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    assert_eq!(s.config().disabled_apps(), Resolved { value: apps(&[]), source: Default, locked: false });

    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_disabled_apps" value="calendar, parking"/>"#)).unwrap();
    assert_eq!(c.disabled_apps(), Resolved { value: apps(&["calendar", "parking"]), source: Provisioned, locked: false });
    // A local list comes before a provisioned one, an empty list too.
    let c = s.update_opendeskphone_settings(|d| d.set_disabled_apps(Some(&apps(&["status"])))).unwrap();
    assert_eq!(c.disabled_apps(), Resolved { value: apps(&["status"]), source: Local, locked: false });
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("disabled_apps {\n  apps {\n    name: \"status\"\n  }\n}\n"));
    let c = s.update_opendeskphone_settings(|d| d.set_disabled_apps(Some(&[]))).unwrap();
    assert_eq!(c.disabled_apps(), Resolved { value: apps(&[]), source: Local, locked: false });
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("disabled_apps {}\n"));

    // Frozen by the server: the local list has no effect.
    let c = s
        .apply_fetched(&provisioned(r#"<setting id="opendeskphone_disabled_apps" value="queues" conflict="freeze"/>"#))
        .unwrap();
    assert_eq!(c.disabled_apps(), Resolved { value: apps(&["queues"]), source: Provisioned, locked: true });
    // A provisioned empty value is nothing set.
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_disabled_apps" value=""/>"#)).unwrap();
    assert_eq!(c.disabled_apps().source, Local);
    let c = s.update_opendeskphone_settings(|d| d.set_disabled_apps(None)).unwrap();
    assert_eq!(c.disabled_apps(), Resolved { value: apps(&[]), source: Default, locked: false });
    assert!(s.set("opendeskphone_disabled_apps", "calendar").is_err());
}

#[test]
fn theme_sources() {
    use opendeskphone_config::{Theme, ThemeChoice};
    use SettingSource::*;
    let (dir, nv, mut s) = store();
    let builtin = |name: &str, source, locked, fallback: &str| Theme {
        choice: ThemeChoice::Builtin(name.into()),
        source,
        locked,
        builtin: fallback.into(),
    };
    let picture = |source, locked, fallback: &str| Theme { choice: ThemeChoice::Picture, source, locked, builtin: fallback.into() };
    // Nothing set: the default theme (no name).
    assert_eq!(s.config().theme(), builtin("", Default, false, ""));
    assert!(!s.config().palette_file_named());

    // A name chosen on the phone is kept in the settings file, and comes
    // back after a restart.
    let c = s.update_opendeskphone_settings(|d| d.theme = Some(" Dusk ".into())).unwrap();
    assert_eq!(c.theme(), builtin("dusk", Local, false, "dusk"));
    assert!(nv.read_string(FILE).unwrap().unwrap().contains("theme: \" Dusk \""));
    assert_eq!(reopen(&dir, &nv).config().theme(), builtin("dusk", Local, false, "dusk"));
    // It is not a stock setting ID.
    assert!(s.set("opendeskphone_theme", "tides").is_err());

    // A provisioned name: the name chosen on the phone goes first, unless
    // the server froze its own.
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_theme" value="Tides"/>"#)).unwrap();
    assert_eq!(c.theme(), builtin("dusk", Local, false, "dusk"));
    let c = s.update_opendeskphone_settings(|d| d.theme = None).unwrap();
    assert_eq!(c.theme(), builtin("tides", Provisioned, false, "tides"));
    // An empty name on the phone is no choice.
    let c = s.update_opendeskphone_settings(|d| d.theme = Some("  ".into())).unwrap();
    assert_eq!(c.theme(), builtin("tides", Provisioned, false, "tides"));
    let c = s.update_opendeskphone_settings(|d| d.theme = Some("slate".into())).unwrap();
    assert_eq!(c.theme(), builtin("slate", Local, false, "slate"));
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_theme" value="tides" conflict="freeze"/>"#)).unwrap();
    assert_eq!(c.theme(), builtin("tides", Provisioned, true, "slate"));
    // A name the configuration layer does not judge: the UI shows the
    // default theme for one it does not have.
    let c = s.apply_fetched(&provisioned(r#"<setting id="opendeskphone_theme" value="no-such"/>"#)).unwrap();
    assert_eq!(c.theme(), builtin("slate", Local, false, "slate"));

    // A provisioned picture (stock's setting, as a DPMA server sends it:
    // frozen) goes before everything; unfrozen, after the phone's choice;
    // and before a provisioned name.
    let file = r#"<setting id="wallpaper_file" value="user" url="file://wall.png" path="/user_wallpaper.png" conflict="freeze"/>"#;
    let c = s.apply_fetched(&provisioned(file)).unwrap();
    assert_eq!(c.theme(), picture(Provisioned, true, "slate"));
    assert!(!c.palette_file_named());
    let unfrozen = r#"<setting id="wallpaper_file" value="user" url="http://pbx/wall.png"/><setting id="opendeskphone_theme" value="tides"/>"#;
    let c = s.apply_fetched(&provisioned(unfrozen)).unwrap();
    assert_eq!(c.theme(), builtin("slate", Local, false, "slate"));
    let c = s.update_opendeskphone_settings(|d| d.theme = None).unwrap();
    assert_eq!(c.theme(), picture(Provisioned, false, "tides"));
    // The setting without a URL names no picture.
    let c = s.apply_fetched(&provisioned(r#"<setting id="wallpaper_file" value="user" url=""/>"#)).unwrap();
    assert_eq!(c.theme(), builtin("", Default, false, ""));

    // The palette file is named beside the picture.
    let both = format!(r#"{file}<setting id="opendeskphone_palette_file" value="user" url="file://wall.txtpb" conflict="freeze"/>"#);
    let c = s.apply_fetched(&provisioned(&both)).unwrap();
    assert_eq!(c.theme(), picture(Provisioned, true, ""));
    assert!(c.palette_file_named());
    assert_eq!(c.setting_attr("opendeskphone_palette_file", "url"), Some("file://wall.txtpb"));
}
