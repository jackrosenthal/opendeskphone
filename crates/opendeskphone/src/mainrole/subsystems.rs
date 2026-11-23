//! Start-up of the subsystems that live in other crates.
//!
//! This is the integration point.  The order is the dependency order and
//! should be kept.  Every step is contained: a subsystem that fails to start
//! is logged, shown in `dphone health`/`dphone state`, and skipped; the SSH
//! server and the diag socket (started before this) stay up regardless.
//!
//! Every subsystem gets a clone of [`Context`] and from it:
//!
//! - `ctx.store`: the state store, event bus, and command bus;
//! - `ctx.diag.register(...)` to add `dphone <command>` diagnostics (the
//!   command is a clap type; see [`crate::diag::Registry`]);
//! - `ctx.health.require(name)` / `satisfy(name)` for anything that must be
//!   up before a trial boot is confirmed (`net` today, `ui` to come);
//! - `ctx.request_shutdown(...)` for reboots, and `ctx.request_exit(...)`
//!   for a restart of the main role only;
//! - `ctx.recovery` to know nvdata is a stand-in tmpfs;
//! - `ctx.sim` to pick the simulator HAL backend;
//! - `ctx.safe`: safe mode, in which only the network (with defaults when the
//!   configuration cannot be read) is started here.
//!
//! All network I/O belongs on the current (single-threaded) runtime via
//! `tokio::spawn`.  The audio thread and the UI thread are plain OS threads
//! started by their subsystems.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{HealthStatus, Subsystem};
use log::{error, info, warn};

use super::{Context, Options};
use crate::diag::Output;

pub mod alarms;
pub mod apphttp;
pub mod appstore;
pub mod apps;
pub mod audio;
pub mod blfitems;
pub mod bluetooth;
pub mod ehs;
pub mod expansion;
pub mod hal;
pub mod liveness;
pub mod net;
pub mod paging;
pub mod provisioning;
pub mod remote;
pub mod alarmtones;
pub mod ringtones;
pub mod syslog;
pub mod system;
pub mod telephony;
pub mod themesync;
pub mod trust;
pub mod ui;
pub mod wallpaper;
#[cfg(feature = "web")]
pub mod web;

/// The configuration store, shared between subsystems.  Calls are short and
/// synchronous; nothing holds the lock across an await.
pub type SharedConfig = Arc<Mutex<ConfigStore>>;

pub async fn start(ctx: &Context, opts: &Options) {
    // 1. opendeskphone-state.  The store is in the context from the start; here the
    //    log ring is bridged into it and the system facts are filled in.
    start_state(ctx);

    // 2. opendeskphone-config: load and merge the nvdata XML layers.
    let hub = system::ConfigHub::default();
    let config = start_config(ctx, &hub);

    // 3. opendeskphone-net: `NetManager` on this runtime.  Before the hardware, so
    //    that nothing there can delay the way in.
    let net = net::start(ctx, config.clone(), &hub);
    {
        let net = net.clone();
        // The network task maps the merged configuration (network, time,
        // and expansion settings) onto its own and acts only on a
        // difference there, so every change is passed on.
        hub.on_change(move |_cfg| net.config_changed());
    }

    // Subsystem liveness, and the kernel's crash records from before this boot.
    liveness::start(ctx);

    // Remote syslog, as soon as the network can carry it.
    {
        let cfg = config.as_ref().map(|c| c.lock().unwrap().config());
        let syslog = syslog::start(ctx, net.clone(), cfg.as_deref());
        hub.on_change(move |cfg| syslog.apply_config(cfg));
    }

    // The SSH switch: opens or closes the listener, now and on every later
    // configuration change.  SSH is off unless `ssh_enabled` says otherwise,
    // in safe mode as at any other time.
    apply_ssh_switch(ctx, config.as_ref());
    hub.on_change(|cfg| crate::ssh::set_enabled(ssh_enabled(cfg)));

    if ctx.safe {
        warn!("safe mode: hardware, audio, telephony, UI, apps, and web are not started");
        ctx.diag.register_unavailable::<hal::HalCommand>("not started (safe mode)");
        system::start(ctx, system::Parts { config, hub, net, mtd: None });
        // No UI and no telephony: there is no setup to show.
        finish(ctx, None);
        return;
    }

    // 4. opendeskphone-hal: `phone` backend, or `sim` when `ctx.sim`.
    let hal = hal::start(ctx).await;

    // The flash goes to the update handlers.
    let mtd: Option<system::SharedMtd> = hal.as_ref().and_then(|h| {
        let mut h = h.lock().ok()?;
        if h.missing.iter().any(|(name, _)| *name == "mtd") {
            return None;
        }
        Some(Arc::new(Mutex::new(std::mem::replace(&mut h.mtd, Box::new(opendeskphone_hal::Null("mtd"))))))
    });

    // 5. opendeskphone-ui: the UI thread.  (It takes the display, keys, and lamps
    //    out of the hardware bundle; `dphone hal` then only reports status.)
    //    It needs only the hardware and the configuration, and starts
    //    before everything that takes time, so that its boot screen and
    //    loading bar are up while the rest starts: the bar follows the
    //    health each subsystem below reports.
    ringtones::init(&ctx.paths.nvdata, &ctx.paths.ringtones);
    alarmtones::init(&ctx.paths.alarm_tones);
    let ui = ui::start(ctx, hal.clone(), config.clone(), &hub).await;

    // 6. opendeskphone-vpm / opendeskphone-media: the DSP, then the audio thread.
    let audio = audio::start(ctx, hal.clone()).await;
    if let Some(cfg) = config.as_ref().map(|c| c.lock().unwrap().config()) {
        audio.apply_config(&cfg);
    }
    {
        let audio = audio.clone();
        hub.on_change(move |cfg| audio.apply_config(cfg));
    }

    // UART A: the EHS headset adapter's by default; the serial console's
    // when `serial_console` is on.
    {
        let ehs = ehs::start(ctx, hal.clone());
        match config.as_ref().map(|c| c.lock().unwrap().config()) {
            Some(cfg) => ehs.apply_config(&cfg),
            None => ehs.no_config(),
        }
        hub.on_change(move |cfg| ehs.apply_config(cfg));
    }

    // 7. The main role's own command domains: Config, Network, Update, System.
    let firmware_mtd = mtd.clone();
    system::start(ctx, system::Parts { config: config.clone(), hub: hub.clone(), net: net.clone(), mtd });

    // Provisioned certificates, kept in nvdata for the TLS users below.
    trust::start(ctx, config.clone(), &hub);

    // 8. opendeskphone-sip and opendeskphone-dpma: accounts, provisioning.
    let telephony = telephony::start(ctx, config.clone(), &hub, net.clone(), audio.clone());
    // Provisioned firmware: downloaded over HTTP(S) and written by the
    // updater.  The DPMA client does not exist yet (it starts once the
    // phone has an address and SIP runs), so the updater is handed over
    // when it does, before its first provisioning pass.  (Without a
    // configuration there is no DPMA client at all.)  An HTTPS server is
    // checked against the trust bundle, like every other download.
    if let (Some(mtd), Some(config)) = (firmware_mtd, config.clone()) {
        let source = provisioning::http_source(&net, config);
        let installer = opendeskphone_dpma::install::MtdInstaller {
            mtd,
            model: super::state_model(ctx.model),
            cmdline: std::fs::read_to_string("/proc/cmdline").unwrap_or_default(),
            own_dir: ctx.paths.own_dir(),
            store: ctx.store.clone(),
        };
        telephony.when_dpma(move |dpma| dpma.set_firmware_updater(source, Arc::new(installer)));
    }

    remote::register(ctx, ui.as_ref().map(|u| u.tap.clone()));

    // The PBX's store of keys and values (Open DPMA's app data): the
    // apps', and the firmware's for the user's theme, which follows the
    // user to a reset phone.
    let app_store = appstore::AppStore::new(telephony.clone(), tokio::runtime::Handle::current());
    app_store.follow(ctx.store.clone());
    themesync::start(ctx, config.clone(), &hub, Arc::new(app_store.clone()), app_store.reached());

    // opendeskphone-apps: the WASM runtime, on its own thread.
    apps::start(ctx, ui.as_ref().map(|u| u.handle.clone()), config.clone(), telephony.clone(), net.clone(), app_store);

    // 9. opendeskphone-web: HTTP(S) and Connect RPC.
    #[cfg(feature = "web")]
    web::start(ctx, opts, config.clone(), &hub, ui.as_ref().map(|u| u.tap.clone()));
    #[cfg(not(feature = "web"))]
    let _ = opts;

    // Bluetooth headsets (D65).
    bluetooth::start(ctx, audio.clone());

    // Expansion modules (new family).
    {
        let expansion = expansion::start(ctx);
        if let Some(cfg) = config.as_ref().map(|c| c.lock().unwrap().config()) {
            expansion.apply_config(&cfg);
        }
        hub.on_change(move |cfg| expansion.apply_config(cfg));
    }

    // Multicast paging.
    {
        let paging = paging::start(ctx, audio.clone());
        if let Some(cfg) = config.as_ref().map(|c| c.lock().unwrap().config()) {
            paging.apply_config(&cfg);
        }
        hub.on_change(move |cfg| paging.apply_config(cfg));
    }

    // The alarm clock.
    {
        let alarms = alarms::start(ctx, audio.clone());
        if let Some(cfg) = config.as_ref().map(|c| c.lock().unwrap().config()) {
            alarms.apply_config(&cfg);
        }
        hub.on_change(move |cfg| alarms.apply_config(cfg));
    }

    let _ = (audio, hal, telephony, ui);
    tokio::spawn(provisioning::follow_setup(ctx.store.clone()));
    finish(ctx, config.as_ref());
}

/// Whether the SSH server should listen.  This is the one place that reads
/// the switch from the configuration; the SSH module only takes the answer
/// (`crate::ssh::set_enabled`).  SSH is off by default; the switch is
/// `ssh_enabled` in `/nvdata/opendeskphone/settings.txtpb`, or the provisioned
/// `opendeskphone_ssh_enabled`.
fn ssh_enabled(config: &opendeskphone_config::Config) -> bool {
    config.ssh_enabled().value
}

/// Set the SSH switch at start.  When the configuration could not be
/// loaded, the settings file alone decides (it does not depend on the XML
/// merge), so that a phone whose owner turned SSH on stays reachable then.
fn apply_ssh_switch(ctx: &Context, config: Option<&SharedConfig>) {
    let enabled = match config {
        Some(c) => ssh_enabled(&c.lock().unwrap().config()),
        None => std::fs::read_to_string(ctx.paths.nvdata.join(opendeskphone_config::settings::SETTINGS_FILE))
            .ok()
            .and_then(|text| opendeskphone_config::SettingsDocument::parse(&text).ok())
            .and_then(|doc| doc.settings.ssh_enabled)
            .unwrap_or(false),
    };
    crate::ssh::set_enabled(enabled);
}

/// Start-up is over: the phone is in recovery, in its setup (no saved
/// provisioning server and no accounts, as after a factory reset; see
/// `provisioning::starts_in_setup`), or running.
fn finish(ctx: &Context, config: Option<&SharedConfig>) {
    use opendeskphone_state::Phase;
    let setup = config.is_some_and(|c| {
        let store = c.lock().unwrap();
        provisioning::starts_in_setup(&store.config(), store.nvdata().read("configurator.xml").ok().flatten().as_deref())
    });
    let phase = if ctx.recovery.is_some() {
        Phase::Recovery
    } else if setup {
        info!("setup: no saved server and no accounts; the phone starts at the server menu");
        Phase::Provisioning
    } else {
        Phase::Running
    };
    ctx.store.update::<opendeskphone_state::System, _>(|s| s.phase = phase);
}

fn start_state(ctx: &Context) {
    let store = ctx.store.clone();
    crate::logging::set_sink(Box::new(move |level, target, message| {
        store.log_line(level.into(), target, message.to_string());
    }));
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let boot_rev = cmdline
        .split_ascii_whitespace()
        .find_map(|w| w.strip_prefix("digium_boot_rev="))
        .and_then(|v| v.parse().ok());
    let mem_total_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.split_ascii_whitespace().next().and_then(|n| n.parse().ok()))
        })
        .unwrap_or(0);
    let last_reason = std::fs::read_to_string(ctx.paths.reboot_reason()).unwrap_or_default();
    ctx.store.update::<opendeskphone_state::System, _>(|s| {
        s.simulated = ctx.sim;
        s.boot_rev = boot_rev;
        s.mem_total_kib = mem_total_kib;
        s.last_reboot_reason = last_reason.trim().to_string();
    });
    ctx.store.set_health(Subsystem::Core, HealthStatus::Ok);

    let store = ctx.store.clone();
    ctx.diag.register(move |args: StateCommand, out: Output| {
        let store = store.clone();
        async move {
            let text = format!("{:#?}\n", store.snapshot());
            match &args.topic {
                None => out.out(&text).await,
                Some(filter) => out.out(&filter_section(&text, filter)).await,
            }
            0
        }
    });
}

/// Dump the state store
#[derive(Debug, Parser)]
#[command(name = "state")]
pub struct StateCommand {
    /// Only the top-level fields whose name contains this (e.g., audio)
    topic: Option<String>,
}

/// Read and change the configuration
#[derive(Debug, Parser)]
#[command(name = "config")]
pub struct ConfigCommand {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the value of one setting
    Get { id: String },
    /// List the settings as ID=VALUE (secrets are masked)
    List {
        /// Only the settings whose ID contains this
        substring: Option<String>,
    },
    /// Set a setting and apply it
    Set {
        id: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Merge the configuration files again and apply the result
    Reload,
}

/// From a `{:#?}` dump of the whole state, keep the top-level fields whose
/// name contains `filter` (case-insensitive).
fn filter_section(text: &str, filter: &str) -> String {
    let filter = filter.to_ascii_lowercase();
    let mut out = String::new();
    let mut keep = false;
    for line in text.lines() {
        // Top-level fields are indented by exactly four spaces.
        if line.starts_with("    ") && !line.starts_with("     ") {
            let name = line.trim().split(':').next().unwrap_or("");
            if line.trim().contains(':') {
                keep = name.to_ascii_lowercase().contains(&filter);
            }
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.is_empty() {
        out = format!("no state field matches '{filter}'\n");
    }
    out
}

fn config_model(ctx: &Context) -> opendeskphone_config::Model {
    ctx.model.into()
}

/// True when this image may write files that stock also owns.
pub fn stock_writes_allowed(ctx: &Context) -> bool {
    ctx.sim || crate::paths::stock_writes_allowed()
}

fn start_config(ctx: &Context, hub: &system::ConfigHub) -> Option<SharedConfig> {
    ctx.store.set_health(Subsystem::Config, HealthStatus::Starting);
    let nv = Nvdata::new(ctx.paths.nvdata.clone());
    let read_only = !stock_writes_allowed(ctx);
    // On a phone the init role performs a requested factory reset right
    // after mounting nvdata.  The simulator has no init role above it.
    if ctx.sim {
        crate::init::nvdata::run_pending_factory_reset(&ctx.paths.nvdata);
    }
    let model = config_model(ctx);
    let tmp = ctx.paths.run.clone();
    // The merge is plain computation over files; run it where a panic in it
    // cannot take the main role down.
    let opened = std::thread::Builder::new()
        .name("config-open".into())
        .spawn(move || if read_only { ConfigStore::open_read_only(nv, model, tmp) } else { ConfigStore::open(nv, model, tmp) })
        .map_err(|e| e.to_string())
        .and_then(|h| h.join().map_err(|_| "panic while merging the configuration".to_string()))
        .and_then(|r| r.map_err(|e| e.to_string()));
    let store = match opened {
        Ok(s) => s,
        Err(e) => {
            error!("config: {e}; continuing with built-in defaults for the network");
            ctx.store.set_health(Subsystem::Config, HealthStatus::Failed(e.clone()));
            ctx.store.update::<opendeskphone_state::ConfigInfo, _>(|c| c.error = e);
            ctx.diag.register_unavailable::<ConfigCommand>("the configuration could not be loaded; see 'dphone log'");
            return None;
        }
    };
    for w in store.warnings() {
        warn!("config: layer skipped: {w}");
    }
    let cfg = store.config();
    info!(
        "config: generation {}, {} settings{}",
        cfg.generation(),
        cfg.settings().count(),
        if read_only { " (stock files left untouched)" } else { "" }
    );
    ctx.store.update::<opendeskphone_state::ConfigInfo, _>(|c| {
        c.generation = cfg.generation();
        c.last_merge_unix = opendeskphone_state::clock::unix_secs();
        c.error.clear();
    });
    ctx.store.update::<opendeskphone_state::System, _>(|s| s.timezone = cfg.time().time_zone);
    ctx.store.set_health(Subsystem::Config, HealthStatus::Ok);

    let shared: SharedConfig = Arc::new(Mutex::new(store));
    let handle = shared.clone();
    let state = ctx.store.clone();
    let hub = hub.clone();
    ctx.diag.register(move |args: ConfigCommand, out: Output| {
        let handle = handle.clone();
        let state = state.clone();
        let hub = hub.clone();
        async move {
            // `set` and `reload` write nvdata: off the runtime thread.
            let (code, text, err, changed) = {
                let handle = handle.clone();
                tokio::task::spawn_blocking(move || config_command(&handle, read_only, &args.action))
                    .await
                    .unwrap_or_else(|e| (1, String::new(), format!("dphone config: {e}\n"), None))
            };
            // A change made here is a change like any other: every
            // subsystem that follows the configuration hears of it.
            if let Some(cfg) = changed {
                hub.changed(&state, &cfg);
            }
            if !text.is_empty() {
                out.out(&text).await;
            }
            if !err.is_empty() {
                out.err(&err).await;
            }
            code
        }
    });
    Some(shared)
}

/// Run a `dphone config` command.  Returns the exit code, the output, the
/// error text, and the new merged configuration when the command changed it.
fn config_command(
    handle: &SharedConfig,
    read_only: bool,
    args: &ConfigAction,
) -> (i32, String, String, Option<Arc<opendeskphone_config::Config>>) {
    let mut changed = None;
    let (code, text, err) = config_command_inner(handle, read_only, args, &mut changed);
    (code, text, err, changed)
}

fn config_command_inner(
    handle: &SharedConfig,
    read_only: bool,
    args: &ConfigAction,
    changed: &mut Option<Arc<opendeskphone_config::Config>>,
) -> (i32, String, String) {
    let Ok(mut store) = handle.lock() else {
        return (1, String::new(), "dphone config: store poisoned\n".into());
    };
    match args {
        ConfigAction::Get { id } => match store.get(id) {
            Some(v) => (0, format!("{v}\n"), String::new()),
            None => (1, String::new(), format!("dphone config: no setting '{id}'\n")),
        },
        ConfigAction::List { substring } => {
            let filter = substring.as_deref().unwrap_or("");
            let cfg = store.config();
            let mut text = String::new();
            for (id, value) in cfg.settings().filter(|(id, _)| id.contains(filter)) {
                // Never print secrets to a terminal by default.
                let shown = if id.contains("password") || id.contains("secret") { "********" } else { value };
                text.push_str(&format!("{id}={shown}\n"));
            }
            (0, text, String::new())
        }
        ConfigAction::Set { id, value } => {
            if read_only && !id.starts_with("open_") {
                return (
                    1,
                    String::new(),
                    format!(
                        "dphone config: this image leaves stock-owned nvdata files untouched ({}); only open_* settings can be set\n",
                        crate::paths::NO_STOCK_WRITES_FLAG
                    ),
                );
            }
            match store.set(id, value) {
                Ok(cfg) => {
                    *changed = Some(cfg);
                    (0, String::new(), String::new())
                }
                Err(e) => (1, String::new(), format!("dphone config: {e}\n")),
            }
        }
        ConfigAction::Reload => match store.reload() {
            Ok(cfg) => {
                let text = format!("generation {}\n", cfg.generation());
                *changed = Some(cfg);
                (0, text, String::new())
            }
            Err(e) => (1, String::new(), format!("dphone config: {e}\n")),
        },
    }
}

/// Run `f` on its own thread and wait for it without blocking the runtime,
/// so that neither a panic nor a slow device open stalls the main role.
pub async fn contained<T: Send + 'static>(
    name: &str,
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| format!("cannot start thread {name}: {e}"))?;
    match tokio::time::timeout(limit, rx).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(format!("{name} panicked")),
        Err(_) => Err(format!("{name} did not finish within {} s", limit.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_filter() {
        let text = "State {\n    system: System {\n        model: D60,\n    },\n    network: Network {\n        phase: Up,\n    },\n}\n";
        let got = filter_section(text, "net");
        assert!(got.contains("phase: Up"));
        assert!(!got.contains("model: D60"));
        assert!(filter_section(text, "zzz").contains("no state field"));
    }
}
