//! The main role: everything except PID 1's duties, in one process on one
//! single-threaded async runtime.
//!
//! Start-up order is in [`run_async`]; the subsystems that other crates
//! provide are started from [`subsystems::start`].

pub mod health;
pub mod subsystems;

use std::net::SocketAddr;
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{error, info, warn};
use tokio::sync::watch;

use crate::diag::Registry;
use crate::init::{BEAT_ALIVE, BEAT_HEALTHY, BEAT_SAFE};
use crate::model::Model;
use crate::paths::Paths;
use crate::sys::{self, Shutdown};

pub use health::Health;

/// How often the main role beats to the init role.
pub const BEAT_INTERVAL: Duration = Duration::from_secs(2);

/// Command-line options of `dphone main`.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Options {
    /// Run without hardware and without a PID 1 above (host simulator)
    #[arg(long, requires = "nvdata")]
    pub sim: bool,
    /// The model to run as [default: detected, or d60 with --sim]
    #[arg(long, value_name = "MODEL", value_parser = parse_model)]
    pub model: Option<Model>,
    /// The nvdata directory [default: /nvdata]
    #[arg(long, value_name = "DIR")]
    pub nvdata: Option<PathBuf>,
    /// The runtime directory [default: /run/opendeskphone, or .run in the
    /// nvdata directory with --sim]
    #[arg(long, value_name = "DIR")]
    pub run_dir: Option<PathBuf>,
    /// The SSH port, on all addresses, used while ssh_enabled is on
    #[arg(long, value_name = "PORT", conflicts_with = "ssh_listen")]
    pub ssh_port: Option<u16>,
    /// The SSH address (e.g., 127.0.0.1:2222), used while ssh_enabled is
    /// on; the simulator has no SSH server without one
    #[arg(long, value_name = "ADDRESS")]
    pub ssh_listen: Option<SocketAddr>,
    /// Do not start SSH
    #[arg(long)]
    pub no_ssh: bool,
    /// The HTTP port of the web UI [default: 80, or 8080 on loopback with --sim]
    #[arg(long, value_name = "PORT")]
    pub web_port: Option<u16>,
    /// Start only SSH, the network, and diagnostics
    #[arg(long)]
    pub safe: bool,
    /// Run in recovery mode, for this reason (the init role passes it when
    /// it could not mount nvdata)
    #[arg(long, value_name = "REASON")]
    pub recovery: Option<String>,
    /// The descriptor of the pipe to beat to the init role on (the init
    /// role passes it)
    #[arg(long, value_name = "FD")]
    pub heartbeat_fd: Option<i32>,
    /// The built-in ringtones [default: /usr/share/opendeskphone/ringtones]
    #[arg(long, value_name = "DIR", env = "DPHONE_RINGTONES")]
    pub ringtones: Option<PathBuf>,
    /// The alarm tones [default: /usr/share/opendeskphone/alarm-tones]
    #[arg(long, value_name = "DIR", env = "DPHONE_ALARM_TONES")]
    pub alarm_tones: Option<PathBuf>,
    /// The staged built-in apps the simulator runs (see
    /// crates/opendeskphone-apps/README.md) [default: none]
    #[arg(long, value_name = "DIR", env = "DPHONE_APPS_ROOT")]
    pub apps_root: Option<PathBuf>,
    /// The shell for SSH sessions and the web terminal [default: the first
    /// of /bin/brush and /bin/sh that exists]
    #[arg(long, value_name = "PROGRAM", env = "DPHONE_SHELL")]
    pub shell: Option<String>,
    /// Log at the debug level
    #[arg(short, long)]
    pub verbose: bool,
}

fn parse_model(name: &str) -> Result<Model, String> {
    name.parse().map_err(|()| {
        let names: Vec<&str> = Model::ALL.iter().map(|m| m.name()).collect();
        format!("not a model ({})", names.join(", "))
    })
}

impl Options {
    /// Where SSH listens, if the command line says.
    pub fn ssh_address(&self) -> Option<SocketAddr> {
        self.ssh_listen.or(self.ssh_port.map(|port| SocketAddr::from(([0, 0, 0, 0], port))))
    }
}

/// Why the main role is ending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    Running,
    /// Leave with this status; the init role restarts us.
    Code(i32),
}

/// Shared handles every subsystem gets.
#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    pub model: Model,
    /// Host simulator: skip hardware.
    pub sim: bool,
    /// Set when the init role could not mount nvdata (value: the reason).
    /// nvdata is then a tmpfs; show the recovery screen and web page, and
    /// treat nothing as persistent.
    pub recovery: Option<String>,
    /// Safe mode: the init role saw the main role fail repeatedly (or
    /// `--safe` was given).  Only SSH, the network, and diagnostics start.
    pub safe: bool,
    /// The shell for SSH sessions and the web terminal (`--shell`), if not
    /// the first of [`crate::init::SHELLS`] that exists.
    pub shell: Option<String>,
    /// The state store shared by every subsystem.
    pub store: opendeskphone_state::Store,
    pub diag: Registry,
    pub health: Health,
    pub started: Instant,
    exit_tx: Arc<watch::Sender<Exit>>,
    supervised: bool,
}

impl Context {
    /// A context with nothing above it (no init role) and nothing started:
    /// for host tests that run one subsystem against real kernel interfaces,
    /// e.g., the network subsystem in a private network namespace.
    pub fn standalone(paths: Paths, model: Model) -> Context {
        let (exit_tx, _) = watch::channel(Exit::Running);
        Context {
            paths,
            model,
            sim: false,
            recovery: None,
            safe: false,
            shell: None,
            store: opendeskphone_state::Store::new(state_model(model)),
            diag: Registry::new(),
            health: Health::new(),
            started: Instant::now(),
            exit_tx: Arc::new(exit_tx),
            supervised: false,
        }
    }

    /// Ask the main role to exit; the init role restarts it.
    pub fn request_exit(&self, code: i32) {
        let _ = self.exit_tx.send(Exit::Code(code));
    }

    /// Reboot, halt, or power off the phone through the init role, recording
    /// `reason` in the stock `reboot_reason` file.  In the simulator, or when
    /// nothing supervises us, this only ends the main role.
    pub fn request_shutdown(&self, kind: Shutdown, reason: &str) {
        info!("{kind:?} requested: {reason}");
        if self.recovery.is_none() && (self.sim || crate::paths::stock_writes_allowed()) {
            let _ = crate::paths::write_atomic(&self.paths.reboot_reason(), format!("{reason}\n").as_bytes(), 0o644);
        }
        if self.sim || !self.supervised {
            self.request_exit(0);
            return;
        }
        let sig = match kind {
            Shutdown::Reboot => libc::SIGTERM,
            Shutdown::Halt => libc::SIGUSR1,
            Shutdown::PowerOff => libc::SIGUSR2,
            Shutdown::Kexec => sys::kexec_signal(),
        };
        if let Err(e) = sys::kill(1, sig) {
            error!("cannot signal the init role: {e}");
        }
    }
}

/// Entry point of `dphone main`.  Returns the process exit status.
pub fn run(opts: Options) -> i32 {
    let level = if opts.verbose { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    crate::logging::init_ring(level, true);
    // A panic anywhere is logged with its location before the thread (or
    // task) unwinds; the default hook's output would miss the log ring.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            error!("cannot start the async runtime: {e}");
            return 1;
        }
    };
    let code = runtime.block_on(run_async(opts));
    // Do not wait for lingering blocking tasks (e.g., a shell session's I/O).
    runtime.shutdown_timeout(Duration::from_millis(200));
    code
}

/// Our model as the state crate's type.
pub fn state_model(model: Model) -> opendeskphone_state::Model {
    model.into()
}

fn resolve_model(opts: &Options) -> Model {
    if let Some(m) = opts.model {
        return m;
    }
    if !opts.sim {
        if let Some(d) = crate::model::detect() {
            return d.model;
        }
        warn!("model not detected; assuming D60");
    }
    Model::D60
}

/// Move the keys of the stock firmware's SSH key file to
/// `/nvdata/opendeskphone/authorized_keys`, without Digium's keys, if that has
/// not been done (`Nvdata::move_stock_authorized_keys`).  Runs before the SSH
/// server and the web UI read the keys.
fn move_stock_authorized_keys(paths: &Paths) {
    use opendeskphone_config::nvdata::{AUTHORIZED_KEYS_FILE, STOCK_AUTHORIZED_KEYS_FILE};
    match opendeskphone_config::Nvdata::new(&paths.nvdata).move_stock_authorized_keys() {
        Ok(Some(n)) => info!(
            "ssh: moved {n} key(s) from {STOCK_AUTHORIZED_KEYS_FILE} to {AUTHORIZED_KEYS_FILE}, without the stock firmware's keys"
        ),
        Ok(None) => {}
        Err(e) => warn!("ssh: cannot move the keys of {STOCK_AUTHORIZED_KEYS_FILE} to {AUTHORIZED_KEYS_FILE}: {e}"),
    }
}

async fn run_async(opts: Options) -> i32 {
    let mut paths = Paths::new(
        opts.nvdata.clone().unwrap_or_else(|| PathBuf::from(crate::paths::NVDATA)),
        opts.run_dir.clone().unwrap_or_else(|| match (&opts.nvdata, opts.sim) {
            // Simulator default: keep the runtime directory beside nvdata.
            (Some(nv), true) => nv.join(".run"),
            _ => PathBuf::from(crate::paths::RUN_DIR),
        }),
    );
    if let Some(dir) = &opts.ringtones {
        paths.ringtones = dir.clone();
    }
    if let Some(dir) = &opts.alarm_tones {
        paths.alarm_tones = dir.clone();
    }
    paths.apps = opts.apps_root.clone();
    let model = resolve_model(&opts);
    // The firmware of the D40 thru D70 is built without the D80's
    // touch skin (the feature `d80`), and has nothing to draw a D80's
    // screen with.
    if model.family() == crate::model::Family::Touch && !cfg!(feature = "d80") {
        error!("this dphone is built for the D40 thru D70 (without the feature `d80`): it cannot run as a {model}");
        return 1;
    }
    let recovery = opts.recovery.clone().filter(|v| !v.is_empty());
    let beat_fd = opts.heartbeat_fd;
    // The descriptor is ours alone; children must not inherit it.
    // SAFETY: fcntl on a descriptor number the init role gave us.
    if let Some(fd) = beat_fd {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }

    let (exit_tx, mut exit_rx) = watch::channel(Exit::Running);
    let ctx = Context {
        safe: opts.safe,
        shell: opts.shell.clone(),
        store: opendeskphone_state::Store::new(state_model(model)),
        paths,
        model,
        sim: opts.sim,
        recovery,
        diag: Registry::new(),
        health: Health::new(),
        started: Instant::now(),
        exit_tx: Arc::new(exit_tx),
        supervised: beat_fd.is_some(),
    };
    info!(
        "{} {} main role starting: model {}{}{}",
        crate::PRODUCT,
        crate::VERSION,
        ctx.model,
        if ctx.sim { ", simulator" } else { "" },
        if ctx.recovery.is_some() { ", RECOVERY (nvdata not mounted)" } else { "" },
    );
    for dir in [ctx.paths.run.clone(), ctx.paths.own_dir()] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("mkdir {}: {e}", dir.display());
        }
    }
    move_stock_authorized_keys(&ctx.paths);

    // 1. Diagnostics socket, so `dphone log` works from the first moment.
    crate::diag::builtin::register(&ctx);
    match crate::diag::server::bind(&ctx.paths.diag_socket()) {
        Ok(listener) => {
            tokio::spawn(crate::diag::server::serve(listener, ctx.diag.clone()));
        }
        Err(e) => error!("diag socket {}: {e}", ctx.paths.diag_socket().display()),
    }

    // 2. Heartbeat to the init role.
    ctx.health.require("core");
    if let Some(fd) = beat_fd {
        tokio::spawn(heartbeat(fd, ctx.health.clone(), ctx.safe));
    }

    // 3. Signals.
    tokio::spawn(signals(ctx.clone()));

    // 4. SSH, before anything that could fail.  It is off by default and
    //    binds nothing here: the listener opens when the settings, read in
    //    the next step, say `ssh_enabled` (`subsystems::apply_ssh_switch`).
    if let Some(listen) = crate::ssh::listen_address(&ctx, &opts) {
        match crate::ssh::start(&ctx, listen).await {
            Ok(()) => {
                ctx.store.set_health(opendeskphone_state::Subsystem::Ssh, opendeskphone_state::HealthStatus::Ok);
            }
            Err(e) => {
                error!("ssh: {e}");
                ctx.store.set_health(opendeskphone_state::Subsystem::Ssh, opendeskphone_state::HealthStatus::Failed(e.to_string()));
            }
        }
    } else {
        info!("ssh: not started");
    }

    // 5. Subsystems from the other crates, in dependency order.
    subsystems::start(&ctx, &opts).await;

    // 6. Our own part is up.
    ctx.health.satisfy("core");

    // 7. Run until told to stop.
    let code = loop {
        if exit_rx.changed().await.is_err() {
            break 1;
        }
        if let Exit::Code(code) = exit_rx.borrow().clone() {
            break code;
        }
    };
    info!("main role exiting with status {code}");
    let _ = std::fs::remove_file(ctx.paths.diag_socket());
    code
}

/// Write beats to the init role.  This task runs on the same single-threaded
/// runtime as everything else, so a stalled event loop stops the beats.
/// In safe mode "healthy" is never sent: the reduced main role being up says
/// nothing about the phone working, and must not confirm a trial boot.
async fn heartbeat(fd: i32, health: Health, safe: bool) {
    use std::io::Write;
    // SAFETY: the init role opened this descriptor for us; nothing else in
    // this process owns it.
    let mut pipe = unsafe { std::fs::File::from_raw_fd(fd) };
    let _ = sys::set_nonblocking(fd, true);
    let mut tick = tokio::time::interval(BEAT_INTERVAL);
    loop {
        tick.tick().await;
        let beat = match (health.is_healthy(), safe) {
            (true, false) => BEAT_HEALTHY,
            (true, true) => BEAT_SAFE,
            (false, _) => BEAT_ALIVE,
        };
        // A full pipe (init busy) or a closed one is not our problem to fix.
        let _ = pipe.write(&[beat]);
    }
}

async fn signals(ctx: Context) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) =
        (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
    else {
        error!("cannot install signal handlers");
        return;
    };
    loop {
        tokio::select! {
            _ = term.recv() => { info!("SIGTERM"); ctx.request_exit(0); }
            _ = int.recv() => { info!("SIGINT"); ctx.request_exit(0); }
            _ = hup.recv() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Main {
        #[command(flatten)]
        options: Options,
    }

    fn parse(words: &[&str]) -> Result<Options, clap::Error> {
        Main::try_parse_from(std::iter::once(&"main").chain(words)).map(|m| m.options)
    }

    #[test]
    fn options() {
        let o = parse(&["--sim", "--model", "d40", "--nvdata", "/tmp/nv", "--ssh-port", "2222"]).unwrap();
        assert_eq!(o.model, Some(Model::D40));
        assert_eq!(o.ssh_address(), Some("0.0.0.0:2222".parse().unwrap()));
        let o = parse(&["--ssh-listen", "127.0.0.1:2022", "-v"]).unwrap();
        assert_eq!(o.ssh_address(), Some("127.0.0.1:2022".parse().unwrap()));
        assert_eq!(parse(&[]).unwrap().ssh_address(), None);
        assert!(parse(&["--model", "d99"]).is_err());
    }
}
