//! Open Desk Phone's web interface (plan section 11): an HTTP and HTTPS server,
//! Connect RPC services, and the embedded frontend.
//!
//! The integrator builds a [`WebServer`] from the state store and the
//! configuration store, attaches the optional pieces, and awaits
//! [`WebServer::run`] on the main role's tokio runtime:
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! use opendeskphone_web::{WebOptions, WebServer};
//! use std::sync::{Arc, Mutex};
//!
//! let store =
//! opendeskphone_state::Store::new(opendeskphone_state::Model::D60); let nv =
//! opendeskphone_config::Nvdata::new("/nvdata"); let config =
//! opendeskphone_config::ConfigStore::open(nv,
//! opendeskphone_config::Model::D60, "/tmp").unwrap(); let server =
//! WebServer::new(store, Arc::new(Mutex::new(config)), WebOptions::default());
//! server.run().await
//! # }
//! ```
//!
//! What the server needs from other subsystems:
//!
//! - State and commands come from [`opendeskphone_state::Store`]: every action
//!   (dial, reboot, install firmware, inject a key) is a
//!   [`opendeskphone_state::Command`], so the owning subsystem only has to
//!   register its handler.
//! - Configuration is read and written through the shared
//!   [`opendeskphone_config::ConfigStore`]; after a write the server posts
//!   [`opendeskphone_state::Command::ReloadConfig`] so the configuration owner
//!   republishes.
//! - [`ScreenSource`] supplies pictures for the screen mirror.
//! - [`FirmwareSink`] receives an uploaded firmware image
//!   ([`FileFirmwareSink`] is the default).
//! - [`Backup`] makes and restores configuration backups
//!   ([`NvdataBackup`] is the default).
//! - [`ShellHost`] starts the web terminal's shell ([`PtyShellHost`] is the
//!   default: `/bin/brush`, else `/bin/sh`, on a pseudo-terminal).
//! - [`SshControl`] applies the SSH server's switch.  Without one the
//!   setting is only stored (`ssh_enabled` in the settings file) and the
//!   configuration owner is told to reload.
//!
//! Every RPC and upload endpoint needs a session, except logging in.  The
//! static frontend files are served without one (they hold no data).
//!
//! `GET /metrics` is a Prometheus scrape endpoint, also served without a
//! session, on both listeners.  It carries no names, numbers, addresses, or
//! secrets, and it renders at most once a second however often it is
//! asked.  The metric list is in the `metrics` module's source
//! (`src/metrics.rs`); in brief: build information, uptime, health per
//! subsystem, network link and address state, registration state per
//! account index, calls in progress and ended, RTP totals, provisioning,
//! configuration, firmware bank and trial state, apps, flash error
//! counters, memory, load, and the usual `process_*` figures.

mod assets;
mod auth;
mod backup;
pub mod rpc;
mod convert;
mod http;
mod metrics;
mod png;
mod services;
mod shell;
mod sshkeys;
mod tls;

/// Generated code for `proto/`: buffa message types and the Connect
/// service traits.
#[allow(clippy::all, missing_docs, unused_qualifications, dead_code)]
pub mod proto {
    connectrpc::include_generated!();
}
/// The `opendeskphone.v1` package.
pub use proto::opendeskphone::v1 as pb;

pub use backup::{Backup, NvdataBackup};
pub use shell::{PtyShellHost, ShellHost, ShellProcess, ShellSize};

use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{HealthStatus, Store, Subsystem};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Which panel a mirror picture is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MirrorScreen {
    Main,
    /// The D70's rapid-dial display.
    Side,
}

/// Pixel layout of a [`MirrorFrame`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorFormat {
    /// 4 bytes per pixel, `B, G, R, x` in memory (what the color panel
    /// scans out).
    Xrgb8888,
    /// 1 byte per pixel, 0 black to 255 white.
    Gray8,
}

/// A copy of a framebuffer.
#[derive(Clone, Debug)]
pub struct MirrorFrame {
    pub width: u16,
    pub height: u16,
    /// Bytes per row.
    pub stride: usize,
    pub format: MirrorFormat,
    pub pixels: Vec<u8>,
}

/// Where the screen mirror gets its pictures.  On the phone the UI
/// subsystem implements this over what it last flushed; in the simulator
/// it wraps the HAL's `SimHandle`.  Key presses and touches do not go
/// through here: they are sent as [`opendeskphone_state::Command::InjectKey`]
/// and [`opendeskphone_state::Command::InjectTouch`].
pub trait ScreenSource: Send + Sync {
    /// A counter that changes whenever any screen's picture changes.  The
    /// mirror polls it (cheaply) and captures only on change.
    fn generation(&self) -> u64;
    /// The current picture of a screen, or `None` if the model lacks it.
    fn capture(&self, screen: MirrorScreen) -> Option<MirrorFrame>;
}

/// What is known about an uploaded firmware image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirmwareInfo {
    /// Version from the image label (`3_0_0` or `2_9_28`), or empty.
    pub version: String,
    /// Model from the image label (`D60`), or empty.
    pub model: String,
    /// A stock Digium image rather than one of ours.
    pub is_stock: bool,
}

/// Receives a firmware upload as it arrives, so the 40 MiB image is never
/// held in RAM by the web server.
pub trait FirmwareSink: Send + Sync {
    /// Starts an upload.  `size` is the `Content-Length`, when given.
    fn begin(&self, size: Option<u64>) -> io::Result<Box<dyn FirmwareUpload>>;
    /// Drops a finished upload that will not be installed.
    fn discard(&self, path: &Path);
}

/// One upload in progress.  Dropping it without `finish` abandons it.
pub trait FirmwareUpload: Send {
    fn write(&mut self, chunk: &[u8]) -> io::Result<()>;
    /// Completes the upload: the path to give
    /// [`opendeskphone_state::Command::InstallFirmware`], and what the image
    /// is.
    fn finish(self: Box<Self>) -> io::Result<(PathBuf, FirmwareInfo)>;
}

/// Looks at a stored image and says what it is.
pub type FirmwareInspector = dyn Fn(&Path) -> io::Result<FirmwareInfo> + Send + Sync;

/// The default [`FirmwareSink`]: writes the image to a file in a directory
/// (a tmpfs on the phone) and asks an inspector what it is.
pub struct FileFirmwareSink {
    dir: PathBuf,
    inspect: Option<Arc<FirmwareInspector>>,
}

impl FileFirmwareSink {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FileFirmwareSink { dir: dir.into(), inspect: None }
    }
    /// Sets the function that reads the image label (the updater's EFF
    /// parser).  Without one, images are reported with empty fields and
    /// treated as not stock; the updater still enforces `allow_stock`.
    pub fn with_inspector(mut self, f: impl Fn(&Path) -> io::Result<FirmwareInfo> + Send + Sync + 'static) -> Self {
        self.inspect = Some(Arc::new(f));
        self
    }
}

struct FileUpload {
    path: PathBuf,
    file: Option<std::fs::File>,
    inspect: Option<Arc<FirmwareInspector>>,
}

impl FirmwareSink for FileFirmwareSink {
    fn begin(&self, _size: Option<u64>) -> io::Result<Box<dyn FirmwareUpload>> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("firmware-{}.eff", auth::random_token()));
        let file = std::fs::File::create(&path)?;
        Ok(Box::new(FileUpload { path, file: Some(file), inspect: self.inspect.clone() }))
    }
    fn discard(&self, path: &Path) {
        if path.starts_with(&self.dir) {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl FirmwareUpload for FileUpload {
    fn write(&mut self, chunk: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.file.as_mut().expect("open until finish").write_all(chunk)
    }
    fn finish(mut self: Box<Self>) -> io::Result<(PathBuf, FirmwareInfo)> {
        drop(self.file.take());
        let info = match &self.inspect {
            Some(f) => f(&self.path)?,
            None => FirmwareInfo::default(),
        };
        let path = std::mem::take(&mut self.path);
        Ok((path, info))
    }
}

impl Drop for FileUpload {
    fn drop(&mut self) {
        // Abandoned, or the inspector rejected it.
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The SSH server's switch, for the integrator that owns the SSH server.
pub trait SshControl: Send + Sync {
    /// Stores the setting (`ssh_enabled` in
    /// `/nvdata/opendeskphone/settings.txtpb`) and starts or stops the
    /// listener.  Called off the reactor thread, with the configuration store
    /// unlocked.
    fn set_enabled(&self, enabled: bool) -> io::Result<()>;
    /// Whether the server is accepting connections right now, when known.
    fn listening(&self) -> Option<bool> {
        None
    }
}

/// Server settings.
/// The two switches of the web server, both changeable while it runs; the
/// server listens whatever they say.
///
/// - Web configuration (stock's `web_ui_enabled`).  Off, the frontend's
///   files are still served so that it can say that web configuration is
///   disabled, but there is no login and no RPC other than
///   `AuthService/GetSession` (which reports the state), no upload, and so
///   no shell and no screen mirror.  Switching off also ends every session,
///   shell, and open connection.
/// - The metrics endpoint (`metrics_enabled` in the Open Desk Phone settings).
///   Off, `GET /metrics` answers 404.
///
/// The two are independent.
#[derive(Clone)]
pub struct WebSwitch {
    web: Arc<tokio::sync::watch::Sender<bool>>,
    metrics: Arc<std::sync::atomic::AtomicBool>,
}

impl WebSwitch {
    /// Web configuration as given; the metrics endpoint on.
    pub fn new(enabled: bool) -> Self {
        WebSwitch {
            web: Arc::new(tokio::sync::watch::Sender::new(enabled)),
            metrics: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    /// Both switches as a merged configuration has them.
    pub fn from_config(config: &opendeskphone_config::Config) -> Self {
        let switch = WebSwitch::new(true);
        switch.follow(config);
        switch
    }

    /// Take both switches from a merged configuration.
    pub fn follow(&self, config: &opendeskphone_config::Config) {
        self.set(config.access().web_ui_enabled);
        self.set_metrics(config.metrics_enabled().value);
    }

    pub fn set(&self, enabled: bool) {
        self.web.send_if_modified(|v| std::mem::replace(v, enabled) != enabled);
    }

    pub fn enabled(&self) -> bool {
        *self.web.borrow()
    }

    pub fn set_metrics(&self, enabled: bool) {
        self.metrics.store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn metrics_enabled(&self) -> bool {
        self.metrics.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolves at the next switch to off.
    pub(crate) async fn turned_off(&self) {
        let mut rx = self.web.subscribe();
        loop {
            if rx.changed().await.is_err() {
                return std::future::pending().await;
            }
            if !*rx.borrow() {
                return;
            }
        }
    }
}

impl Default for WebSwitch {
    fn default() -> Self {
        WebSwitch::new(true)
    }
}

#[derive(Clone, Debug)]
pub struct WebOptions {
    /// Plain HTTP listener; `None` to disable.
    pub http: Option<SocketAddr>,
    /// HTTPS listener; `None` to disable.
    pub https: Option<SocketAddr>,
    /// Where uploads (app packages; firmware with the default sink) land.
    /// A tmpfs on the phone.
    pub upload_dir: PathBuf,
    /// Sessions end after this long without a request.
    pub session_idle: Duration,
    /// Largest firmware upload accepted.
    pub max_firmware_bytes: u64,
    /// Largest app package accepted.
    pub max_app_bytes: u64,
    /// Connections served at once; more are refused.
    pub max_connections: usize,
}

impl Default for WebOptions {
    fn default() -> Self {
        WebOptions {
            http: Some(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 80))),
            https: Some(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 443))),
            upload_dir: PathBuf::from("/tmp/upload"),
            session_idle: Duration::from_secs(30 * 60),
            max_firmware_bytes: 96 << 20,
            // The largest app file the phone takes
            // (`opendeskphone_apps::install::MAX_APP_FILE`).
            max_app_bytes: 8 << 20,
            max_connections: 24,
        }
    }
}

/// An upload waiting for its install RPC.
pub(crate) struct PendingUpload {
    pub path: PathBuf,
    pub size: u64,
    pub firmware: Option<FirmwareInfo>,
}

/// Everything the request handlers share.
pub(crate) struct App {
    pub store: Store,
    pub config: Arc<Mutex<ConfigStore>>,
    pub options: WebOptions,
    pub switch: WebSwitch,
    pub sessions: auth::Sessions,
    pub screen: Option<Arc<dyn ScreenSource>>,
    pub firmware: Box<dyn FirmwareSink>,
    pub backup: Box<dyn Backup>,
    pub shells: shell::Shells,
    pub ssh: Option<Arc<dyn SshControl>>,
    pub uploads: Mutex<HashMap<String, PendingUpload>>,
    pub started: std::time::Instant,
    /// What `/metrics` keeps between scrapes.
    pub metrics: metrics::Metrics,
    /// The Connect services as a tower service.
    pub rpc: connectrpc::ConnectRpcService,
}

/// The web server.  See the crate documentation.
pub struct WebServer {
    store: Store,
    config: Arc<Mutex<ConfigStore>>,
    options: WebOptions,
    screen: Option<Arc<dyn ScreenSource>>,
    firmware: Option<Box<dyn FirmwareSink>>,
    backup: Option<Box<dyn Backup>>,
    shell: Option<Arc<dyn ShellHost>>,
    ssh: Option<Arc<dyn SshControl>>,
    switch: WebSwitch,
}

impl WebServer {
    pub fn new(store: Store, config: Arc<Mutex<ConfigStore>>, options: WebOptions) -> Self {
        WebServer { store, config, options, screen: None, firmware: None, backup: None, shell: None, ssh: None, switch: WebSwitch::default() }
    }

    /// The switches for web configuration and the metrics endpoint (both
    /// on unless given).
    pub fn with_switch(mut self, switch: WebSwitch) -> Self {
        self.switch = switch;
        self
    }

    /// Attaches the screen mirror's picture source.
    pub fn with_screen(mut self, screen: Arc<dyn ScreenSource>) -> Self {
        self.screen = Some(screen);
        self
    }

    /// Replaces the default firmware sink (a file under `upload_dir`).
    pub fn with_firmware_sink(mut self, sink: Box<dyn FirmwareSink>) -> Self {
        self.firmware = Some(sink);
        self
    }

    /// Replaces the default backup implementation ([`NvdataBackup`]).
    pub fn with_backup(mut self, backup: Box<dyn Backup>) -> Self {
        self.backup = Some(backup);
        self
    }

    /// Replaces how the web terminal's shell is started ([`PtyShellHost`]'s
    /// default by default).
    pub fn with_shell(mut self, host: Arc<dyn ShellHost>) -> Self {
        self.shell = Some(host);
        self
    }

    /// Attaches the SSH server's switch.
    pub fn with_ssh_control(mut self, ssh: Arc<dyn SshControl>) -> Self {
        self.ssh = Some(ssh);
        self
    }

    /// Binds the listeners.  Fails if a configured port cannot be bound.
    pub async fn bind(self) -> io::Result<BoundServer> {
        let nv = self.config.lock().unwrap().nvdata().clone();
        let firmware = self.firmware.unwrap_or_else(|| {
            Box::new(FileFirmwareSink::new(self.options.upload_dir.clone()))
        });
        let backup = self.backup.unwrap_or_else(|| Box::new(NvdataBackup::new(nv.clone())));
        let metrics = metrics::Metrics::new(&self.store, nv.root().to_path_buf());
        let http = match self.options.http {
            Some(addr) => Some(http::listen(addr)?),
            None => None,
        };
        let https = match self.options.https {
            Some(addr) => Some(http::listen(addr)?),
            None => None,
        };
        let app = Arc::new(App {
            sessions: auth::Sessions::new(self.options.session_idle),
            switch: self.switch,
            store: self.store,
            config: self.config,
            options: self.options,
            screen: self.screen,
            firmware,
            backup,
            shells: shell::Shells::new(self.shell.unwrap_or_else(|| Arc::new(PtyShellHost::default()))),
            ssh: self.ssh,
            uploads: Mutex::new(HashMap::new()),
            started: std::time::Instant::now(),
            metrics,
            rpc: connectrpc::ConnectRpcService::new(services::router()).with_limits(
                connectrpc::Limits::default()
                    .with_max_request_body_size(http::MAX_RPC_BODY)
                    .with_max_message_size(http::MAX_RPC_BODY),
            ),
        });
        Ok(BoundServer { app, http, https, nv })
    }

    /// Binds and serves until the task is dropped.
    pub async fn run(self) -> io::Result<()> {
        self.bind().await?.serve().await
    }
}

/// A server whose ports are bound but which is not yet accepting.
pub struct BoundServer {
    app: Arc<App>,
    http: Option<tokio::net::TcpListener>,
    https: Option<tokio::net::TcpListener>,
    /// Where the HTTPS key and certificate are kept.
    nv: Nvdata,
}

impl BoundServer {
    /// The bound HTTP address (useful with port 0 in tests).
    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http.as_ref().and_then(|l| l.local_addr().ok())
    }

    pub fn https_addr(&self) -> Option<SocketAddr> {
        self.https.as_ref().and_then(|l| l.local_addr().ok())
    }

    /// Serves until the task is dropped.  A failure to set up HTTPS (key
    /// generation, storage) is logged and leaves plain HTTP running.
    pub async fn serve(self) -> io::Result<()> {
        let BoundServer { app, http, https, nv } = self;
        let store = app.store.clone();
        store.set_health(Subsystem::Web, HealthStatus::Starting);

        let tls = match https {
            Some(listener) => {
                let hostname = store.get::<opendeskphone_state::System>().hostname.clone();
                // Key generation and signing take a moment on the phone.
                let made = tokio::task::spawn_blocking(move || tls::server_config(&nv, &hostname)).await;
                match made {
                    Ok(Ok(cfg)) => Some((listener, cfg)),
                    Ok(Err(e)) => {
                        log::error!("HTTPS disabled: {e}");
                        None
                    }
                    Err(e) => {
                        log::error!("HTTPS disabled: {e}");
                        None
                    }
                }
            }
            None => None,
        };
        let degraded = app.options.https.is_some() && tls.is_none();
        if http.is_none() && tls.is_none() {
            store.set_health(Subsystem::Web, HealthStatus::Failed("no listener".into()));
            return Err(io::Error::other("web server has no listener"));
        }
        store.set_health(
            Subsystem::Web,
            if degraded { HealthStatus::Degraded("HTTPS unavailable".into()) } else { HealthStatus::Ok },
        );

        let limit = Arc::new(tokio::sync::Semaphore::new(app.options.max_connections));
        let plain = async {
            match http {
                Some(l) => http::accept_loop(app.clone(), l, None, limit.clone()).await,
                None => std::future::pending().await,
            }
        };
        let secure = async {
            match tls {
                Some((l, cfg)) => http::accept_loop(app.clone(), l, Some(cfg), limit.clone()).await,
                None => std::future::pending().await,
            }
        };
        let beat = async {
            loop {
                store.heartbeat(Subsystem::Web);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        };
        // Switched off: nobody stays logged in and no shell stays open.
        // (Open connections end by themselves; see `http::serve`.)
        let switched_off = async {
            loop {
                app.switch.turned_off().await;
                log::info!("web configuration disabled");
                app.sessions.clear();
                app.shells.close_all();
            }
        };
        tokio::select! {
            r = plain => r,
            r = secure => r,
            _ = beat => Ok(()),
            _ = switched_off => Ok(()),
            _ = app.metrics.run() => Ok(()),
        }
    }
}

/// Whether this build embeds the real frontend (`web/dist` existed at
/// build time) rather than the placeholder page.
pub fn has_frontend() -> bool {
    assets::have_frontend()
}
