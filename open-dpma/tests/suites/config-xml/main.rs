//! The config-xml suite: smoke checks of the documents the configuration
//! builds.
//!
//! ```text
//! cargo xtask test config-xml
//! ```
//!
//! runs it in the test container after building the devtest module, as
//!
//! ```text
//! cargo test -p dpma-tests --test config-xml -- --ignored [--no-prepare] [-v] [PART...]
//! ```
//!
//! The suite prepares the instance with the setup of
//! [`dpma_tests::harness::setup::config_xml`] (`--no-prepare` skips this).
//! `-v` prints the documents.
//!
//! No expected documents are recorded (they were compared with the
//! official DPMA's by hand); the suite checks that every document the setup
//! describes is produced and well formed, and what a reload does with
//! options that were removed or misspelled.

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{bail, Result};
use bstr::{BString, ByteSlice};
use dpma_tests::harness::{fixtures, setup, ConfFile, Ctx, Suite};
use dpma_tests::xml;

/// Values that the official DPMA writes as they are in the configuration,
/// which makes a document that is not well formed (attribute text as in
/// the document), by phone: an `&` that begins no reference, a `<`, and a
/// `"` are escaped; a `>` and a reference written in the file stay.
/// (`fullphone` has display rules, so libxml2 writes its document, with
/// `>` as a reference too.)
const ESCAPED: [(&str, &[&str]); 2] = [
    (
        "fullphone",
        &[
            r#"display_name="Remote &amp; &lt;Other&gt;""#,
            r#"<setting id="idle_company_text" value="ACME &lt;Corp&gt; &amp; &quot;Sons&quot; R&amp;D"/>"#,
        ],
    ),
    (
        "nolinephone",
        &[r#"display_name="Remote &amp; &lt;Other>""#],
    ),
];

/// `text` with every empty element written as libxml2 writes one (`<a/>`,
/// not `<a />`), and ending in a line break: the form in which a document
/// that display rules were merged into is compared with one that the
/// module wrote directly.
fn plain(text: impl AsRef<[u8]>) -> BString {
    let mut text: BString = text.as_ref().replace(" />", "/>").into();
    if !text.ends_with(b"\n") {
        text.push(b'\n');
    }
    text
}

/// The output of a `dpmadev` command (printed with `-v`); fails when the
/// module has no `dpmadev` commands.
fn document(ctx: &Ctx, cmd: &str) -> Result<BString> {
    let out = ctx.cli(cmd);
    if out.contains_str("No such command") {
        bail!("dpmadev hooks not available (devtest build installed and Asterisk running?)");
    }
    if ctx.verbose {
        ctx.c.say(out.to_str_lossy());
    }
    Ok(out)
}

/// The first `n` bytes of `doc` as text.
fn head(doc: &[u8], n: usize) -> String {
    doc[..doc.len().min(n)].to_str_lossy().into_owned()
}

fn config(ctx: &mut Ctx) -> Result<()> {
    for phone in ["fullphone", "minphone", "altphone", "nolinephone"] {
        let raw = document(ctx, &format!("dpmadev config {phone} D65"))?;
        let doc = plain(&raw);
        ctx.c.check(
            format!("{phone}: a configuration document"),
            doc.contains_str("<config>") && doc.contains_str("</config>"),
            head(&doc, 200),
        );
        let escaped = ESCAPED
            .iter()
            .find(|(p, _)| *p == phone)
            .map_or(&[][..], |(_, t)| *t);
        for text in escaped {
            ctx.c.check(
                format!("{phone}: escaped {text}"),
                doc.contains_str(text),
                "",
            );
        }
        let err = xml::well_formed(&doc);
        ctx.c.check(
            format!("{phone}: well formed"),
            err.is_ok(),
            err.err().unwrap_or_default(),
        );
        // The locale setting only for a phone with an active_locale (the
        // stock firmware's boot configuration fails on an empty one).
        let locale: &[u8] = if phone == "fullphone" {
            b"<setting id=\"locale\" value=\"de_DE\" conflict=\"freeze\"/>\n"
        } else {
            b""
        };
        ctx.c.check(
            format!("{phone}: the locale setting"),
            doc.find_iter("<setting id=\"locale\"").count() == usize::from(!locale.is_empty())
                && doc.contains_str(locale),
            "",
        );
        // The phones with display rules.  fullphone has texts that the
        // official DPMA writes unescaped, so that libxml2 cannot read the
        // document and the rules are not merged.
        if phone == "altphone" || phone == "fullphone" {
            ctx.c.check(
                format!("{phone}: display rules merged"),
                raw.starts_with(b"<?xml version=\"1.0\"?>\n<config>"),
                head(&doc, 80),
            );
        }
    }
    // The deprecated network_vlan_qos of network3, which has no rtp_qos.
    let doc = plain(document(ctx, "dpmadev config fullphone D65")?);
    ctx.c.check(
        "fullphone: network_vlan_qos sets the RTP priority",
        doc.contains_str("<setting id=\"rtp_qos\" value=\"4\" network_id=\"network3\"/>\n")
            && doc.contains_str("<setting id=\"rtp_qos\" value=\"5\" network_id=\"network1\"/>\n"),
        find_all(&doc, "<setting id=\"rtp_qos\"").join(" | "),
    );
    opendeskphone_options(ctx)?;
    let doc = document(ctx, "dpmadev config cfgphone D65")?;
    ctx.c.check(
        "cfgphone: the config_file",
        doc == "<config>\n<setting id=\"x\" value=\"1\" />\n</config>\n",
        format!("{doc:?}"),
    );
    migration(ctx)?;
    Ok(())
}

/// The migration documents (`dpmadev config PHONE MODEL migrate`) contain
/// the payload of `crate::migrate` in `lan_port_mode`, an emptied
/// `pc_port_mode` (a `down` value would lead the stock daemon away from
/// the setting), and no firmware offer, which a stock phone could only
/// refuse.  The rest of the document is the ordinary one.
fn migration(ctx: &mut Ctx) -> Result<()> {
    for phone in ["fullphone", "altphone"] {
        let raw = document(ctx, &format!("dpmadev config {phone} D80 migrate"))?;
        let doc = plain(&raw);
        ctx.c.check(
            format!("{phone}: the migration payload in lan_port_mode"),
            doc.contains_str(
                "<setting id=\"lan_port_mode\" value=\"auto;sh /data/digium/tmp/odp-migrate.sh\"/>",
            ),
            find_all(&doc, "<setting id=\"lan_port_mode\"").join(" | "),
        );
        ctx.c.check(
            format!("{phone}: pc_port_mode emptied"),
            doc.contains_str("<setting id=\"pc_port_mode\" value=\"\"/>"),
            find_all(&doc, "<setting id=\"pc_port_mode\"").join(" | "),
        );
        ctx.c.check(
            format!("{phone}: no firmware offered"),
            !doc.contains_str("<firmware ") && !doc.contains_str("<firmwares>"),
            find_all(&doc, "<firmware").join(" | "),
        );
        let err = xml::well_formed(&doc);
        ctx.c.check(
            format!("{phone}: well formed"),
            err.is_ok(),
            err.err().unwrap_or_default(),
        );
        ctx.c.check(
            format!("{phone}: the rest of the document"),
            doc.contains_str("<setting id=\"server_environment\" value=\"DPMA\"/>"),
            "",
        );
    }
    Ok(())
}

/// Open Desk Phone's settings as `fullphone` sets them: the yes/no settings,
/// the static address with its gateway and DNS servers (the values that are
/// not addresses are left out, and a list with one is left out whole), the
/// disabled apps (a list with something that is not an app name is left out
/// whole), the idle softkeys (a list with something that is neither a
/// function nor an app is left out whole), the theme (a value that is
/// not a name is left out, and the name is in lower case), and
/// the two acceptable keys of the six configured (a duplicate, a private
/// key header, a line with options, and a short blob are left out).  Each
/// setting is frozen, so that a value set on the phone does not go before
/// the configuration's.
const OPENDESKPHONE_SETTINGS: &str = concat!(
    "<setting id=\"opendeskphone_ssh_enabled\" value=\"0\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_ignore_provisioned_firmware\" value=\"1\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_metrics_enabled\" value=\"0\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_serial_console\" value=\"1\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_static_address\" value=\"192.0.2.20/23\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_static_gateway\" value=\"192.0.2.1\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_static_dns\" value=\"192.0.2.1,192.0.2.2,192.0.2.4\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_disabled_apps\" value=\"calendar,parking,queues\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_idle_softkeys\" value=\"call_log,app:parking,menu,dnd\" conflict=\"freeze\" />\n",
    "<setting id=\"opendeskphone_theme\" value=\"deep-sea\" conflict=\"freeze\" />\n",
);
const OPENDESKPHONE_KEYS: &str = concat!(
    "<opendeskphone_ssh_authorized_keys>\n",
    "<key id=\"1\" value=\"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa alice@laptop\" />\n",
    "<key id=\"2\" value=\"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb bob &lt;desk&gt; &amp; co\" />\n",
    "</opendeskphone_ssh_authorized_keys>\n",
    "</config>\n",
);

/// The lines of `doc` that start with `prefix`.
fn find_all(doc: &[u8], prefix: &str) -> Vec<String> {
    String::from_utf8_lossy(doc)
        .lines()
        .filter(|line| line.starts_with(prefix))
        .map(str::to_owned)
        .collect()
}

/// The Open Desk Phone options: present as configured for `fullphone`, and
/// absent from the document of a phone that does not set them.
fn opendeskphone_options(ctx: &mut Ctx) -> Result<()> {
    let doc = plain(document(ctx, "dpmadev config fullphone D65")?);
    ctx.c.check(
        "fullphone: Open Desk Phone settings",
        doc.contains_str(plain(OPENDESKPHONE_SETTINGS)),
        "",
    );
    ctx.c.check(
        "fullphone: Open Desk Phone SSH keys",
        doc.contains_str(plain(OPENDESKPHONE_KEYS)) && !doc.contains_str("PRIVATE KEY"),
        "",
    );
    // The palette file is offered as the wallpaper is: here, not in the
    // file directory, so from each network's file URL prefix.
    let palette = find_all(&doc, "<setting id=\"opendeskphone_palette_file\"");
    ctx.c.check(
        "fullphone: Open Desk Phone palette file",
        !palette.is_empty()
            && palette.len() == find_all(&doc, "<setting id=\"wallpaper_file\"").len()
            && palette.iter().all(|line| {
                line.contains("value=\"user\"")
                    && line.contains("wall.txtpb\" path=\"/wallpaper_palette.txtpb\" conflict=\"freeze\" network_id=\"")
            }),
        palette.join("\n"),
    );
    // A firmware file found on the server has its checksum stated; one
    // named by URL, or not found, has the entry of the official DPMA.
    ctx.c.check(
        "fullphone: checksum of a profile's local file",
        doc.contains_str(
            "1_1_0_0_48178_D40_firmware.eff\" network_id=\"network1\" checksum=\"0a1b2c3d-1500000-ffffffff-39321600\"/>\n",
        ),
        "",
    );
    ctx.c.check(
        "fullphone: no checksum for a URL",
        doc.contains_str(
            "url=\"http://fw.example.com/2_9_17_D65_firmware.eff\" network_id=\"network1\"/>\n",
        ),
        "",
    );
    // altphone has no firmware options, so it is offered the packages;
    // its document is rewritten when the display rules are merged in, so
    // only the attribute is looked for.  One of the two packages with a
    // version is a file in the package directory, and altphone has two
    // networks.
    let doc = document(ctx, "dpmadev config altphone D65")?;
    ctx.c.check(
        "altphone: checksum of the package file that is here, and no other",
        doc.find_iter("checksum=\"0a1b2c3d-1500000-ffffffff-39321600\"")
            .count()
            == 2
            && doc.find_iter("checksum=").count() == 2
            && doc.find_iter("<firmware ").count() == 4,
        doc.lines()
            .filter(|l| l.contains_str("firmware"))
            .map(|l| l.to_str_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" | "),
    );
    for phone in ["minphone", "altphone", "nolinephone"] {
        let doc = document(ctx, &format!("dpmadev config {phone} D65"))?;
        ctx.c.check(
            format!("{phone}: no Open Desk Phone options"),
            !doc.contains_str("opendeskphone_"),
            "",
        );
    }
    Ok(())
}

/// The registration port of each network of the `port*` phones, by
/// phone: the network, its address, the port, and the transport.  A
/// network section without `registration_port` has the default port of
/// the transport that is sent with it (the line's, or else the
/// network's; none is UDP): 5061 for TLS, otherwise 5060.  (The official
/// DPMA sends an empty port.)  A port that is set stays, and so does the
/// port of the network that `[general]` gives every phone.
type Port = (&'static str, &'static str, &'static str, &'static str);
const PORTS: [(&str, &str, &[Port]); 3] = [
    (
        "portphone",
        "",
        &[
            ("port-udp", "10.20.1.1", "5060", "udp"),
            ("port-udp-set", "10.20.2.1", "5071", "udp"),
            ("port-tcp", "10.20.3.1", "5060", "tcp"),
            ("port-tcp-set", "10.20.4.1", "5072", "tcp"),
            ("port-tls", "10.20.5.1", "5061", "tls"),
            ("port-tls-set", "10.20.6.1", "5073", "tls"),
            ("port-none", "10.20.7.1", "5060", ""),
            ("portphone", "192.0.2.10", "5070", ""),
        ],
    ),
    // The line's transport goes before the network's.
    (
        "porttlsphone",
        "tls",
        &[
            ("port-udp", "10.20.1.1", "5061", "tls"),
            ("port-udp-set", "10.20.2.1", "5071", "tls"),
            ("port-none", "10.20.7.1", "5061", "tls"),
            ("porttlsphone", "192.0.2.10", "5070", "tls"),
        ],
    ),
    (
        "porttcpphone",
        "tcp",
        &[
            ("port-tls", "10.20.5.1", "5060", "tcp"),
            ("port-tls-set", "10.20.6.1", "5073", "tcp"),
            ("porttcpphone", "192.0.2.10", "5070", "tcp"),
        ],
    ),
];

/// The registration port that a phone is sent for each of its networks,
/// in the line's account and in the configuration server's URL.
fn ports(ctx: &mut Ctx) -> Result<()> {
    for (phone, line_transport, networks) in PORTS {
        let doc = plain(document(ctx, &format!("dpmadev config {phone} D65"))?);
        let hosts = find_all(&doc, "<host_primary ");
        let urls = find_all(&doc, "<setting id=\"config_server_url\"");
        ctx.c.same(
            format!("{phone}: an account and a server URL for each network"),
            (hosts.len(), urls.len()),
            (networks.len(), networks.len()),
        );
        for (network, address, port, transport) in networks {
            // No transport at all is sent as UDP, and not named in a URL.
            let (name, option) = match *transport {
                "" => ("udp", String::new()),
                t => (t, format!(";transport={t}")),
            };
            let host = format!(
                "<host_primary server=\"{address}\" port=\"{port}\" transport=\"{name}\"/>"
            );
            let of = if line_transport.is_empty() {
                String::new()
            } else {
                format!(" (line transport {line_transport})")
            };
            ctx.c.check(
                format!("{phone}: {network} registers to port {port}{of}"),
                hosts.iter().filter(|h| **h == host).count() == 1,
                hosts.join(" | "),
            );
            let url = format!(
                "<setting id=\"config_server_url\" value=\"sip:proxy@{address}:{port}{option}\" network_id=\"{network}\"/>"
            );
            ctx.c.check(
                format!("{phone}: {network} has the server URL with port {port}{of}"),
                urls.contains(&url),
                urls.join(" | "),
            );
        }
        ctx.c.check(
            format!("{phone}: no empty port"),
            !doc.contains_str("port=\"\"") && !doc.contains_str(":\" network_id="),
            "",
        );
    }
    Ok(())
}

/// The `[general]` options of the instance's `res_digium_phone.conf` set
/// to `options` (`None` removes one), and the module reloaded.
fn set_options(ctx: &Ctx, section: &str, options: &[(&str, Option<&str>)]) -> Result<()> {
    let opts: Vec<(&str, Option<&[u8]>)> = options
        .iter()
        .map(|(k, v)| (*k, v.map(str::as_bytes)))
        .collect();
    ConfFile::new(&ctx.inst.conf("res_digium_phone.conf")).set_options(section, &opts)?;
    ctx.cli("module reload res_open_dpma.so");
    Ok(())
}

/// What a reload does with options that changed in the file.
fn reload(ctx: &mut Ctx) -> Result<()> {
    let backup = ConfFile::new(&ctx.inst.conf("res_digium_phone.conf")).backup()?;
    let res = reload_checks(ctx);
    backup.restore()?;
    ctx.cli("module reload res_open_dpma.so");
    res
}

fn reload_checks(ctx: &mut Ctx) -> Result<()> {
    // enable_check_sync is off again once its line is gone.
    let check_sync = |ctx: &Ctx| -> Result<Vec<String>> {
        let doc = plain(document(ctx, "dpmadev config fullphone D65")?);
        Ok(find_all(&doc, "<setting id=\"enable_check_sync\""))
    };
    let setting = |v: u8| format!("<setting id=\"enable_check_sync\" value=\"{v}\"/>");
    ctx.c
        .same("enable_check_sync=yes", check_sync(ctx)?, vec![setting(1)]);
    set_options(ctx, "fullphone", &[("enable_check_sync", None)])?;
    ctx.c.same(
        "enable_check_sync removed from the section",
        check_sync(ctx)?,
        vec![setting(0)],
    );

    // A config_auth or userlist_auth value that is none of the values is
    // an error, and show settings says that requests are refused (the
    // request-handlers suite sends the requests).  Corrected, the values
    // apply again.
    let settings = |ctx: &Ctx| {
        let out = ctx.cli_text("digium_phones show settings");
        let line = |name: &str| {
            out.lines()
                .find_map(|l| l.strip_prefix(name))
                .unwrap_or("?")
                .trim()
                .to_owned()
        };
        (
            line("Userlist Authentication:"),
            line("Config Authentication:"),
        )
    };
    ctx.c.same(
        "the setup's authentication",
        settings(ctx),
        ("globalpin".to_owned(), "mac globalpin".to_owned()),
    );
    let mark = ctx.log().mark();
    set_options(
        ctx,
        "general",
        &[
            ("config_auth", Some("mac_globalpn")),
            ("userlist_auth", Some("globalpn")),
        ],
    )?;
    let config_line = conf_line(ctx, "config_auth=mac_globalpn")?;
    let userlist_line = conf_line(ctx, "userlist_auth=globalpn")?;
    ctx.c.same(
        "misspelled values refuse requests",
        settings(ctx),
        (
            format!("invalid value 'globalpn' at line {userlist_line}, requests refused"),
            format!("invalid value 'mac_globalpn' at line {config_line}, requests refused"),
        ),
    );
    for text in [
        format!("Invalid value 'mac_globalpn' for config_auth at line {config_line} of res_digium_phone.conf: configuration token requests are refused until the value is disabled, mac, pin, globalpin, mac_pin, or mac_globalpin"),
        format!("Invalid value 'globalpn' for userlist_auth at line {userlist_line} of res_digium_phone.conf: user list requests are refused until the value is disabled or globalpin"),
    ] {
        let ok = logged_error(ctx, mark, &text);
        ctx.c.check(format!("logged as an error: {text}"), ok, "not in the log");
    }
    set_options(
        ctx,
        "general",
        &[
            ("config_auth", Some("mac_globalpin")),
            ("userlist_auth", Some("globalpin")),
        ],
    )?;
    ctx.c.same(
        "corrected values apply",
        settings(ctx),
        ("globalpin".to_owned(), "mac globalpin".to_owned()),
    );

    // An invalid value of an on/off option that opens a way into the
    // phone turns the feature off, and is an error.  (fullphone sets
    // opendeskphone_serial_console=yes.)
    let mark = ctx.log().mark();
    let access = [
        ("opendeskphone_ssh_enabled", "nope"),
        ("opendeskphone_serial_console", "yse"),
        ("opendeskphone_metrics_enabled", "ture"),
    ];
    let opts: Vec<(&str, Option<&str>)> = access.iter().map(|(k, v)| (*k, Some(*v))).collect();
    set_options(ctx, "fullphone", &opts)?;
    let doc = plain(document(ctx, "dpmadev config fullphone D65")?);
    for (option, value) in access {
        let setting = format!("<setting id=\"{option}\" value=\"0\" conflict=\"freeze\"/>\n");
        ctx.c.check(
            format!("{option}={value}: off"),
            doc.contains_str(&setting),
            find_all(&doc, &format!("<setting id=\"{option}\"")).join(" | "),
        );
        let line = conf_line(ctx, &format!("{option}={value}"))?;
        let text = format!(
            "Invalid value '{value}' for {option} at line {line} of res_digium_phone.conf: phone 'fullphone' is sent {option}=no"
        );
        let ok = logged_error(ctx, mark, &text);
        ctx.c
            .check(format!("logged as an error: {text}"), ok, "not in the log");
    }
    Ok(())
}

/// The line of the instance's `res_digium_phone.conf` that is `text`.
fn conf_line(ctx: &Ctx, text: &str) -> Result<usize> {
    let conf = std::fs::read_to_string(ctx.inst.conf("res_digium_phone.conf"))?;
    match conf.lines().position(|l| l == text) {
        Some(i) => Ok(i + 1),
        None => bail!("res_digium_phone.conf has no line {text:?}"),
    }
}

/// Whether `text` was logged at the ERROR level since `mark` (waiting up
/// to two seconds for Asterisk's logger).
fn logged_error(ctx: &Ctx, mark: dpma_tests::harness::LogMark, text: &str) -> bool {
    let log = ctx.log();
    dpma_tests::util::wait_until(Duration::from_secs(2), Duration::from_millis(20), || {
        log.since(mark)
            .lines()
            .any(|l| l.contains_str("ERROR[") && l.ends_with(text.as_bytes()))
    })
}

fn contacts(ctx: &mut Ctx) -> Result<()> {
    let doc = document(ctx, "dpmadev contacts")?;
    let err = xml::well_formed(&doc);
    let ok = doc.starts_with(b"<contacts ") && err.is_ok();
    ctx.c.check(
        "contacts: well formed",
        ok,
        err.err().unwrap_or_else(|| head(&doc, 200)),
    );
    Ok(())
}

fn userlist(ctx: &mut Ctx) -> Result<()> {
    for (args, count) in [("", 0), (" 344486", 8)] {
        let out = document(ctx, &format!("dpmadev userlist{args}"))?;
        let (_, body) = out.split_once_str("\n").unwrap_or((&out, b""));
        let users = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["users"].as_array().map(Vec::len));
        ctx.c.check(
            format!("userlist{args}: {count} user(s)"),
            users == Some(count),
            head(&out, 200),
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    Suite::new("config-xml")
        .prepare(|inst| fixtures::prepare(inst, setup::config_xml))
        .part("config", config)
        .part("ports", ports)
        .part("contacts", contacts)
        .part("userlist", userlist)
        .part("reload", reload)
        .main()
}
