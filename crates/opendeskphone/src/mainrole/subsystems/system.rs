//! The command domains the main role itself answers: `Config`, `Network`,
//! `Update`, and `System`; and the hub that tells every subsystem when the
//! merged configuration changes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opendeskphone_config::Config;
use opendeskphone_hal::Mtd;
use opendeskphone_state::{Command, CommandError, Domain, Event, Reply, Store, Subsystem, UpdatePhase};
use opendeskphone_update::{Bank, Updater, trial};
use log::{error, info, warn};

use super::SharedConfig;
use super::net::NetHandle;
use crate::mainrole::{Context, state_model};
use crate::sys::Shutdown;

type ConfigListener = Box<dyn Fn(&Arc<Config>) + Send + Sync>;

/// Listeners for configuration changes.  Subsystems register a closure when
/// they start; it runs on the runtime thread after every merge that changed
/// something, and must be quick.
#[derive(Clone, Default)]
pub struct ConfigHub {
    listeners: Arc<Mutex<Vec<ConfigListener>>>,
}

impl ConfigHub {
    pub fn on_change(&self, f: impl Fn(&Arc<Config>) + Send + Sync + 'static) {
        self.listeners.lock().unwrap().push(Box::new(f));
    }

    /// A merge finished: publish the generation and tell everyone.
    pub fn changed(&self, store: &Store, cfg: &Arc<Config>) {
        let generation = cfg.generation();
        store.update::<opendeskphone_state::ConfigInfo, _>(|c| {
            c.generation = generation;
            c.last_merge_unix = opendeskphone_state::clock::unix_secs();
            c.error.clear();
        });
        store.update::<opendeskphone_state::System, _>(|s| s.timezone = cfg.time().time_zone);
        for l in self.listeners.lock().unwrap().iter() {
            l(cfg);
        }
        store.emit(Event::ConfigApplied { generation });
    }
}

/// The flash, shared between the update command handler and diagnostics.
pub type SharedMtd = Arc<Mutex<Box<dyn Mtd>>>;

/// What installing an image takes; for the web upload, which goes into
/// the spare bank (or a D80's nvdata) as it arrives.
#[derive(Clone)]
pub struct Flash {
    pub mtd: SharedMtd,
    pub model: opendeskphone_state::Model,
    pub own_dir: std::path::PathBuf,
    pub store: Store,
}

static FLASH: std::sync::OnceLock<Flash> = std::sync::OnceLock::new();

/// The flash of this phone, once the system subsystem has started; `None`
/// when there is none.
pub fn flash() -> Option<Flash> {
    FLASH.get().cloned()
}

/// The image a web upload wrote into the spare bank (or a D80's nvdata)
/// and verified there.  It is not installed: `Command::InstallFirmware`
/// with [`STAGED_PATH`] commits it.
pub static STAGED: Mutex<Option<opendeskphone_update::Staged>> = Mutex::new(None);

/// The `path` of `Command::InstallFirmware` that means the staged image.
pub const STAGED_PATH: &str = "staged:";

pub struct Parts {
    pub config: Option<SharedConfig>,
    pub hub: ConfigHub,
    pub net: NetHandle,
    pub mtd: Option<SharedMtd>,
}

pub fn start(ctx: &Context, parts: Parts) {
    let Parts { config, hub, net, mtd } = parts;
    tokio::spawn(config_domain(ctx.clone(), config.clone(), hub));
    tokio::spawn(network_domain(ctx.store.clone(), net));
    tokio::spawn(system_domain(ctx.clone(), config, mtd.clone()));
    tokio::spawn(update_domain(ctx.clone(), mtd.clone()));
    tokio::spawn(confirm_trial_when_healthy(ctx.clone()));

    if let Some(mtd) = &mtd {
        let _ = FLASH.set(Flash {
            mtd: mtd.clone(),
            model: state_model(ctx.model),
            own_dir: ctx.paths.own_dir(),
            store: ctx.store.clone(),
        });
        // An upload that waited to be installed went with the system that
        // received it (on a D80, it would fill nvdata).
        if let Ok(mut m) = mtd.lock() {
            Updater::new(m.as_mut(), state_model(ctx.model), "", &ctx.paths.own_dir()).discard_staged();
        }
    }
    // Seed the update state: running bank and trial status.
    if let Some(mtd) = mtd {
        let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
        if let Ok(m) = mtd.lock() {
            trial::seed_state(&ctx.store, &ctx.paths.own_dir(), &cmdline, m.as_ref());
        }
    }
}

async fn config_domain(ctx: Context, config: Option<SharedConfig>, hub: ConfigHub) {
    let rx = ctx.store.handle(Domain::Config);
    let read_only = !super::stock_writes_allowed(&ctx);
    while let Some(envelope) = rx.next().await {
        let (cmd, responder) = envelope.split();
        let Some(config) = &config else {
            responder.reply(Err(CommandError::Failed("the configuration could not be loaded".into())));
            continue;
        };
        // Every one of these writes nvdata (the merged `config.xml` at the
        // least), and a JFFS2 write can take long when the volume collects
        // garbage: they run on the blocking pool, not on the runtime thread
        // that carries the network and the heartbeat.
        let result = match cmd {
            Command::ReloadConfig | Command::ReloadContacts => {
                // (Contact lists are read from disk by their users; a reload
                // of the merge picks up changed sources.)
                write_config(config, |store| store.reload()).await
            }
            Command::SetSettings { settings } => {
                if read_only && settings.iter().any(|(id, _)| !id.starts_with("open_")) {
                    Err(CommandError::Denied(
                        "this image leaves stock-owned nvdata files untouched; settings cannot be saved".into(),
                    ))
                } else {
                    write_config(config, move |store| {
                        let pairs: Vec<(&str, &str)> = settings.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                        store.set_many(&pairs)
                    })
                    .await
                }
            }
            Command::SetTheme { name } => {
                // One of Open Desk Phone's own settings: its file is not one
                // of stock's, so a read-only image saves it too.
                let locked = config.lock().unwrap().config().theme().locked;
                if locked {
                    Err(CommandError::Denied("the provisioning server fixed the theme".into()))
                } else {
                    let name = name.trim().to_ascii_lowercase();
                    write_config(config, move |store| store.update_opendeskphone_settings(|s| s.theme = (!name.is_empty()).then_some(name))).await
                }
            }
            other => Err(CommandError::Invalid(format!("not a configuration command: {other:?}"))),
        };
        match result {
            Ok(cfg) => {
                hub.changed(&ctx.store, &cfg);
                responder.reply(Ok(Reply::Done));
            }
            Err(e) => {
                if let CommandError::Failed(text) = &e {
                    warn!("config: {text}");
                    ctx.store.update::<opendeskphone_state::ConfigInfo, _>(|c| c.error = text.clone());
                }
                responder.reply(Err(e));
            }
        }
    }
}

/// Run a configuration-store operation that writes nvdata on the blocking
/// pool and wait for it without holding up the runtime thread.
pub async fn write_config<T: Send + 'static>(
    config: &SharedConfig,
    op: impl FnOnce(&mut opendeskphone_config::ConfigStore) -> Result<T, opendeskphone_config::ConfigError> + Send + 'static,
) -> Result<T, CommandError> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = config.lock().map_err(|_| CommandError::Failed("configuration store poisoned".into()))?;
        op(&mut store).map_err(command_error)
    })
    .await
    .unwrap_or_else(|e| Err(CommandError::Failed(format!("configuration write did not finish: {e}"))))
}

/// A change the store turns away (a frozen setting, or one that is not
/// stock's) is refused, as a change on a read-only image is: nothing was
/// merged, so it is not the failed merge that `ConfigInfo::error` reports.
fn command_error(e: opendeskphone_config::ConfigError) -> CommandError {
    use opendeskphone_config::ConfigError;
    match e {
        ConfigError::Frozen(_) | ConfigError::NotAStockSetting(_) => CommandError::Denied(e.to_string()),
        e => CommandError::Failed(e.to_string()),
    }
}

async fn network_domain(store: Store, net: NetHandle) {
    let rx = store.handle(Domain::Network);
    while let Some(envelope) = rx.next().await {
        let (cmd, responder) = envelope.split();
        let result = match cmd {
            // A renewal is a fresh bring-up with the same settings.
            Command::RenewDhcp | Command::ReconfigureNetwork => {
                net.reconfigure();
                Ok(Reply::Done)
            }
            Command::SyncTime => {
                net.sync_time();
                Ok(Reply::Done)
            }
            other => Err(CommandError::Invalid(format!("not a network command: {other:?}"))),
        };
        responder.reply(result);
    }
}

/// Erase the nvdata partition.  Only in recovery, where the partition could
/// not be mounted and `/nvdata` is a stand-in: JFFS2 mounts an erased
/// partition as an empty volume at the next boot.
fn erase_nvdata(ctx: &Context, mtd: Option<&SharedMtd>) -> Result<(), CommandError> {
    if ctx.recovery.is_none() {
        return Err(CommandError::Denied("nvdata is mounted; use a factory reset instead".into()));
    }
    let mtd = mtd.ok_or_else(|| CommandError::Failed("flash is not available".into()))?;
    let mut mtd = mtd.lock().map_err(|_| CommandError::Failed("flash lock poisoned".into()))?;
    let mut part = mtd.open("nvdata").map_err(|e| CommandError::Failed(e.to_string()))?;
    let bad = opendeskphone_hal::erase_all(part.as_mut(), |_, _| {}).map_err(|e| CommandError::Failed(e.to_string()))?;
    warn!("recovery: nvdata erased ({bad} bad block(s) skipped)");
    Ok(())
}

async fn system_domain(ctx: Context, config: Option<SharedConfig>, mtd: Option<SharedMtd>) {
    let rx = ctx.store.handle(Domain::System);
    while let Some(envelope) = rx.next().await {
        let (cmd, responder) = envelope.split();
        let result = match cmd {
            Command::Reboot { reason } => {
                let reason = if reason.is_empty() { "requested".to_string() } else { reason };
                shutdown_soon(&ctx, reason);
                Ok(Reply::Done)
            }
            Command::FactoryReset => {
                if !super::stock_writes_allowed(&ctx) {
                    Err(CommandError::Denied("this image leaves stock-owned nvdata files untouched".into()))
                } else {
                    // Stock's marker file: the init role performs the reset
                    // at the next boot, right after mounting nvdata
                    // (`init::nvdata::run_pending_factory_reset`).
                    let nv = match &config {
                        Some(c) => c.lock().unwrap().nvdata().clone(),
                        None => opendeskphone_config::Nvdata::new(ctx.paths.nvdata.clone()),
                    };
                    match tokio::task::spawn_blocking(move || nv.request_factory_reset()).await {
                        Ok(Ok(())) => {
                            shutdown_soon(&ctx, "factory reset".to_string());
                            Ok(Reply::Done)
                        }
                        Ok(Err(e)) => Err(CommandError::Failed(e.to_string())),
                        Err(e) => Err(CommandError::Failed(e.to_string())),
                    }
                }
            }
            Command::EraseNvdata => {
                let (ctx2, mtd) = (ctx.clone(), mtd.clone());
                match tokio::task::spawn_blocking(move || erase_nvdata(&ctx2, mtd.as_ref())).await {
                    Ok(Ok(())) => {
                        shutdown_soon(&ctx, "nvdata erased".to_string());
                        Ok(Reply::Done)
                    }
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(CommandError::Failed(e.to_string())),
                }
            }
            Command::SetTime { unix_secs } => match crate::sys::set_clock(unix_secs as i64) {
                Ok(()) => {
                    ctx.store.emit(Event::TimeChanged);
                    Ok(Reply::Done)
                }
                Err(e) => Err(CommandError::Failed(e.to_string())),
            },
            other => Err(CommandError::Invalid(format!("not a system command: {other:?}"))),
        };
        responder.reply(result);
    }
}

/// Announce the shutdown, give listeners (UI, SIP unregister) a moment, then
/// go down through the init role.
fn shutdown_soon(ctx: &Context, reason: String) {
    ctx.store.update::<opendeskphone_state::System, _>(|s| s.phase = opendeskphone_state::Phase::ShuttingDown);
    ctx.store.emit(Event::ShuttingDown { reason: reason.clone() });
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        ctx.request_shutdown(Shutdown::Reboot, &format!("dphone: {reason}"));
    });
}

fn cmdline() -> String {
    std::fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

async fn update_domain(ctx: Context, mtd: Option<SharedMtd>) {
    let rx = ctx.store.handle(Domain::Update);
    let model = state_model(ctx.model);
    while let Some(envelope) = rx.next().await {
        let (cmd, responder) = envelope.split();
        let Some(mtd) = mtd.clone() else {
            responder.reply(Err(CommandError::Failed("flash is not available".into())));
            continue;
        };
        match cmd {
            Command::InstallFirmware { path, allow_stock } => {
                if matches!(
                    ctx.store.get::<opendeskphone_state::Update>().phase,
                    UpdatePhase::Idle | UpdatePhase::Complete | UpdatePhase::Failed(_)
                ) {
                    // The write takes minutes: do it on its own thread and
                    // answer when it is done.  Progress is in the store.
                    let store = ctx.store.clone();
                    let own_dir = ctx.paths.own_dir();
                    let spawned = std::thread::Builder::new().name("update".into()).spawn(move || {
                        responder.reply(install(&store, &mtd, model, &path, allow_stock, &own_dir));
                    });
                    if let Err(e) = spawned {
                        error!("update: cannot start the update thread: {e}");
                    }
                } else {
                    responder.reply(Err(CommandError::Invalid("an update is already running".into())));
                }
            }
            Command::ConfirmTrial => {
                let r = trial::mark_healthy(&ctx.paths.own_dir(), Some(&ctx.store));
                responder.reply(r.map(|()| Reply::Done).map_err(|e| CommandError::Failed(e.to_string())));
            }
            Command::Rollback => {
                let result = (|| {
                    let mut m = mtd.lock().map_err(|_| "flash lock poisoned".to_string())?;
                    let parts = m.partitions().map_err(|e| e.to_string())?;
                    let running = opendeskphone_update::bank::running_bank(&cmdline(), &parts)
                        .ok_or("not running from either bank; use 'dphone update switch-bank'")?;
                    let other = match running {
                        Bank::A => Bank::B,
                        Bank::B => Bank::A,
                    };
                    let _lock = opendeskphone_update::lock::acquire(&ctx.paths.own_dir()).map_err(|e| e.to_string())?;
                    opendeskphone_update::select_bank(m.as_mut(), model, other).map_err(|e| e.to_string())?;
                    let _ = trial::clear(&ctx.paths.own_dir());
                    Ok::<(), String>(())
                })();
                match result {
                    Ok(()) => {
                        shutdown_soon(&ctx, "switching to the other firmware bank".into());
                        responder.reply(Ok(Reply::Done));
                    }
                    Err(e) => responder.reply(Err(CommandError::Failed(e))),
                }
            }
            other => responder.reply(Err(CommandError::Invalid(format!("not an update command: {other:?}")))),
        }
    }
}

pub(super) fn install(
    store: &Store,
    mtd: &SharedMtd,
    model: opendeskphone_state::Model,
    path: &str,
    allow_stock: bool,
    own_dir: &std::path::Path,
) -> Result<Reply, CommandError> {
    let stock_refusal = |version: &str| {
        CommandError::Denied(format!(
            "{version} is stock firmware; installing it replaces Open Desk Phone and must be asked for explicitly"
        ))
    };
    if path == STAGED_PATH {
        // A web upload: the image is in the spare bank (or a D80's
        // nvdata) already, verified and not installed.  The commit checks
        // it once more.
        let mut slot = STAGED.lock().map_err(|_| CommandError::Failed("staged image lock poisoned".into()))?;
        let staged = slot.as_ref().ok_or_else(|| CommandError::Failed("no uploaded image is waiting; upload it again".into()))?;
        if staged.is_stock && !allow_stock {
            return Err(stock_refusal(&staged.version));
        }
        let mut m = mtd.lock().map_err(|_| CommandError::Failed("flash lock poisoned".into()))?;
        let installed = Updater::new(m.as_mut(), model, &cmdline(), own_dir)
            .store(store.clone())
            .commit(staged)
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        *slot = None;
        info!("update: installed {} {}", installed.version, installed.target);
        return Ok(Reply::Text(installed.version));
    }
    let mut file = std::fs::File::open(path)
        .map(|f| std::io::BufReader::with_capacity(64 * 1024, f))
        .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?;
    // Look at the label first: a stock image is only written when asked for.
    let header = eff::EffReader::new(&mut file)
        .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?
        .header()
        .clone();
    if opendeskphone_update::is_stock_version(&header.version) && !allow_stock {
        return Err(stock_refusal(&header.version));
    }
    let mut m = mtd.lock().map_err(|_| CommandError::Failed("flash lock poisoned".into()))?;
    // A file: read and checked in full before anything is written.
    let installed = Updater::new(m.as_mut(), model, &cmdline(), own_dir)
        .store(store.clone())
        .install_eff_file(file)
        .map_err(|e| CommandError::Failed(e.to_string()))?;
    info!("update: installed {} {}", installed.version, installed.target);
    Ok(Reply::Text(installed.version))
}

/// The main role's half of trial confirmation (the init role does the same
/// on a healthy heartbeat; either may come first).  Safe mode confirms
/// nothing: it starts neither the UI nor telephony.
async fn confirm_trial_when_healthy(ctx: Context) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if ctx.health.is_healthy() {
            // Not from a system started from RAM (`dphone ramboot`): the
            // image on trial is the one in flash, not the one running.
            if !ctx.sim && !ctx.safe && ctx.recovery.is_none() && !crate::init::trial::started_from_ram() {
                let _ = trial::mark_healthy(&ctx.paths.own_dir(), Some(&ctx.store));
            }
            ctx.store.heartbeat(Subsystem::Core);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_config::{ConfigStore, Nvdata};
    use opendeskphone_state::ConfigInfo;

    #[tokio::test]
    async fn a_refused_change_is_not_a_configuration_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = Context::standalone(
            crate::paths::Paths::new(dir.path().join("nvdata"), dir.path().join("run")),
            crate::model::Model::D60,
        );
        ctx.sim = true;
        let nv = Nvdata::new(dir.path().join("nvdata"));
        nv.ensure_layout().unwrap();
        let store = ConfigStore::open(nv, opendeskphone_config::Model::D60, dir.path().join("tmp")).unwrap();
        let config: SharedConfig = Arc::new(Mutex::new(store));
        tokio::spawn(config_domain(ctx.clone(), Some(config.clone()), ConfigHub::default()));
        // The domain takes commands once its task has run.
        tokio::task::yield_now().await;
        let set = |id: &str, value: &str| Command::SetSettings { settings: vec![(id.into(), value.into())] };

        // The image's default ringtone is frozen in the base layer.
        let r = ctx.store.send(set("default_ringtone", "Acheron")).await;
        assert!(matches!(r, Err(CommandError::Denied(_))), "{r:?}");
        let r = ctx.store.send(set("opendeskphone_theme", "dusk")).await;
        assert!(matches!(r, Err(CommandError::Denied(_))), "{r:?}");
        assert_eq!(ctx.store.get::<ConfigInfo>().error, "");
        assert_eq!(ctx.store.send(set("active_ringtone", "Acheron")).await, Ok(Reply::Done));
        assert_eq!(config.lock().unwrap().get("active_ringtone").as_deref(), Some("Acheron"));
    }
}
