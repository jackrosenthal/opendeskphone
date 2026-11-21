//! The app manager: discovers apps, runs them, feeds them events, and
//! publishes [`opendeskphone_state::Apps`].
//!
//! It is single-threaded: build it, then either call [`AppManager::run`]
//! on a thread of its own or call [`AppManager::pump`] from an existing
//! loop whenever the waker passed to [`AppManager::set_waker`] fires or
//! the returned delay passes.

use crate::engine::{AppEngine, EPOCH_TICK, EngineError, Target};
use crate::host::bindings::App;
use crate::host::bindings::opendeskphone::app::types as wit;
use crate::host::{self, Action, AppCtx, Budget, IDLE_BIT, Inbound, Inbox, Timer};
use crate::install::{InstallError, Installer};
use crate::manifest::{AppType, Capability, Manifest};
use crate::prepare::{self, Job, Prepared};
use crate::services::Services;
use crate::view::{Shell, WindowEvent, WindowKind};
use opendeskphone_state::{
    AppInfo, Apps, Command, CommandError, CommandReceiver, Domain, Event, Reply, Responder,
    Store, Subscription, Subsystem, Topic,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use wasmtime::UpdateDeadline;
use wasmtime::component::{Component, Linker};

#[derive(Clone, Debug)]
pub struct ManagerConfig {
    /// Built-in apps, read-only: `<root>/<name>/{app.pb,<name>.cwasm}`.
    pub builtin_root: PathBuf,
    /// Installed apps.
    pub installed_root: PathBuf,
    /// Stock JavaScript apps; only looked at to log that they are skipped.
    pub js_root: PathBuf,
    /// The apps' stores of keys and values: `<root>/<name>/<key>`.
    pub nv_root: PathBuf,
    /// Default linear-memory cap per app.
    pub memory_cap: usize,
    /// The most a manifest may ask for.
    pub memory_hard_cap: usize,
    /// Longest the app's code may run in one event handler before the
    /// app is stopped.  Time a host function waits for the phone's answer
    /// does not count.
    pub call_budget: Duration,
    /// Longest the app's code may run in `start`, counted the same way.
    pub start_budget: Duration,
    /// Longest one call of `start` or of an event handler may take in
    /// all, waits for the phone's answer included, before the app is
    /// stopped: while one app's call runs, no other app receives events.
    pub whole_budget: Duration,
    /// How long after a fault an app that starts with the phone is
    /// started again: this long after the first fault, and twice as long
    /// after each further one, up to `restart_delay_max`.
    pub restart_delay: Duration,
    pub restart_delay_max: Duration,
    /// An app that has run this long without a fault starts over at
    /// `restart_delay`.
    pub restart_forgiven: Duration,
    pub target: Target,
}

impl ManagerConfig {
    /// Paths on the phone.
    pub fn phone() -> ManagerConfig {
        ManagerConfig {
            builtin_root: "/usr/share/opendeskphone/apps".into(),
            installed_root: "/nvdata/opendeskphone/wasmapps".into(),
            js_root: "/nvdata/jsapps".into(),
            nv_root: "/nvdata/appdata".into(),
            memory_cap: 4 << 20,
            memory_hard_cap: 16 << 20,
            call_budget: Duration::from_secs(2),
            start_budget: Duration::from_secs(5),
            whole_budget: Duration::from_secs(10),
            restart_delay: Duration::from_secs(10),
            restart_delay_max: Duration::from_secs(3600),
            restart_forgiven: Duration::from_secs(3600),
            target: Target::Host,
        }
    }

    /// The same layout under another root (simulator, tests): `nvdata` is
    /// the nvdata directory, `builtin_root` the built apps.
    pub fn rooted(builtin_root: &Path, nvdata: &Path) -> ManagerConfig {
        ManagerConfig {
            builtin_root: builtin_root.into(),
            installed_root: nvdata.join("opendeskphone/wasmapps"),
            js_root: nvdata.join("jsapps"),
            nv_root: nvdata.join("appdata"),
            ..ManagerConfig::phone()
        }
    }
}

struct Known {
    dir: PathBuf,
    manifest: Manifest,
    builtin: bool,
    code: Option<Component>,
}

struct Running {
    store: wasmtime::Store<AppCtx>,
    app: App,
    /// When the instance started (monotonic milliseconds).
    started_ms: u64,
}

/// The faults of an app that starts with the phone.
#[derive(Default)]
struct Faults {
    /// Faults in a row: none of the instances between them ran for
    /// `restart_forgiven`, and the user did not open the app.
    count: u32,
    /// When the app is started again (monotonic milliseconds).
    restart_at_ms: Option<u64>,
}

/// How long after its `count`th fault in a row an app is started again.
fn restart_delay(cfg: &ManagerConfig, count: u32) -> Duration {
    let doublings = count.saturating_sub(1).min(31);
    cfg.restart_delay
        .saturating_mul(1 << doublings)
        .min(cfg.restart_delay_max)
}

use crate::prepare::CODE_CACHE;

/// What is said of an installed app whose code is being compiled.
const COMPILING: &str = "the app is being compiled for this firmware; try again in a moment";

pub struct AppManager {
    cfg: ManagerConfig,
    engine: AppEngine,
    store: Store,
    shell: Box<dyn Shell>,
    services: Services,
    inbox: Arc<Inbox>,
    installer: Installer,
    known: BTreeMap<String, Known>,
    running: HashMap<String, Running>,
    foreground: Option<String>,
    idle_shown: HashSet<String>,
    commands: CommandReceiver,
    sub: Subscription,
    generation: u64,
    /// The calls as the apps were last told of them.
    last_calls: Vec<wit::CallInfo>,
    /// The apps that got the push of a hidden call (a call that belongs
    /// to apps), by the call's id.
    hidden_calls: HashMap<u32, Vec<String>>,
    last_mwi: Arc<dyn_mwi::Snapshot>,
    last_presence: Arc<opendeskphone_state::Presence>,
    stop: Arc<AtomicBool>,
    external_waker: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Compilations waiting for the worker thread, each with the command
    /// to answer, if a command asked for it.
    jobs: VecDeque<(Job, Option<Responder>)>,
    /// The compilation the worker thread is doing now.
    compiling: Option<(Job, Option<Responder>)>,
    /// The apps that watch their store on the PBX, as the store was last
    /// told.
    store_watch: Vec<String>,
    /// The apps that start with the phone and faulted.
    faults: HashMap<String, Faults>,
}

mod dyn_mwi {
    pub type Snapshot = opendeskphone_state::Mwi;
}

impl AppManager {
    pub fn new(
        cfg: ManagerConfig,
        store: Store,
        shell: Box<dyn Shell>,
        services: Services,
    ) -> Result<AppManager, EngineError> {
        let engine = AppEngine::new(cfg.target)?;
        let installer = Installer {
            builtin_root: cfg.builtin_root.clone(),
            installed_root: cfg.installed_root.clone(),
            nv_root: cfg.nv_root.clone(),
        };
        installer.recover();
        let commands = store.handle(Domain::Apps);
        let sub = store.subscribe_events([
            Topic::Calls,
            Topic::Mwi,
            Topic::Presence,
            Topic::Alarms,
            Topic::Provisioning,
        ]);
        let inbox = Arc::new(Inbox::default());
        {
            let inbox = inbox.clone();
            services
                .store
                .subscribe(Box::new(move |signal| inbox.push(Inbound::StoreSignal(signal))));
        }
        Ok(AppManager {
            last_calls: store.get::<opendeskphone_state::Calls>().calls.iter().map(host::call_info).collect(),
            hidden_calls: HashMap::new(),
            last_mwi: store.get::<opendeskphone_state::Mwi>(),
            last_presence: store.get::<opendeskphone_state::Presence>(),
            cfg,
            engine,
            store,
            shell,
            services,
            inbox,
            installer,
            known: BTreeMap::new(),
            running: HashMap::new(),
            foreground: None,
            idle_shown: HashSet::new(),
            commands,
            sub,
            generation: 0,
            stop: Arc::new(AtomicBool::new(false)),
            external_waker: None,
            jobs: VecDeque::new(),
            compiling: None,
            store_watch: Vec::new(),
            faults: HashMap::new(),
        })
    }

    /// Set to make [`AppManager::run`] return.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// A function that wakes [`AppManager::run`] (or, with
    /// [`AppManager::set_waker`], the caller's loop) from any thread.
    pub fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let n = self.inbox.notifier.clone();
        let ext = self.external_waker.clone();
        Arc::new(move || {
            n.notify();
            if let Some(f) = &ext {
                f();
            }
        })
    }

    /// For callers that drive [`AppManager::pump`] from their own loop:
    /// `f` is called from any thread when there is work.
    pub fn set_waker(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.external_waker = Some(f);
    }

    /// Discovers built-in and installed apps and publishes the list.
    pub fn scan(&mut self) {
        let mut known = BTreeMap::new();
        for (dir, manifest) in Installer::scan_root(&self.cfg.builtin_root) {
            let code = self.known.remove(&manifest.name).and_then(|k| k.code);
            known.insert(
                manifest.name.clone(),
                Known {
                    dir,
                    manifest,
                    builtin: true,
                    code,
                },
            );
        }
        for (dir, manifest) in Installer::scan_root(&self.cfg.installed_root) {
            if known.contains_key(&manifest.name) {
                log::warn!(
                    "installed app {:?} shadows a built-in; ignored",
                    manifest.name
                );
                continue;
            }
            known.insert(
                manifest.name.clone(),
                Known {
                    dir,
                    manifest,
                    builtin: false,
                    code: None,
                },
            );
        }
        // Stock JavaScript apps cannot run; say so once per scan.
        if let Ok(rd) = std::fs::read_dir(&self.cfg.js_root) {
            for e in rd.flatten() {
                if e.path().join("app.json").is_file() {
                    log::info!(
                        "skipping JavaScript app {:?}: it cannot run on this firmware",
                        e.file_name()
                    );
                }
            }
        }
        self.known = known;
        self.publish();
    }

    /// Starts the apps that run without being opened: background apps and
    /// apps with an idle-screen window.
    pub fn autostart(&mut self) {
        self.refresh_code();
        let names: Vec<String> = self
            .known
            .values()
            .filter(|k| k.manifest.kind == AppType::Background || k.manifest.has_idle)
            .filter(|k| !k.builtin || self.services.config.app_enabled(&k.manifest.name))
            .map(|k| k.manifest.name.clone())
            .collect();
        for n in names {
            match self.start(&n) {
                Ok(()) => {}
                // It starts when its code is ready (`finish_recompile`).
                Err(e) if e == COMPILING => {}
                Err(e) => log::warn!("app {n}: could not start: {e}"),
            }
        }
        self.publish();
    }

    /// Has the code of every installed app compiled again whose compiled
    /// code this firmware cannot load: after a firmware update that changed
    /// the engine, or when the file is missing.  The work is done by the
    /// worker thread, one app after the other.
    fn refresh_code(&mut self) {
        if !AppEngine::can_compile() {
            return;
        }
        let stale: Vec<String> = self
            .known
            .iter()
            .filter(|(_, k)| !k.builtin && k.code.is_none())
            // SAFETY: the file is this manager's own (see `load_code`).
            .filter(|(_, k)| {
                unsafe { self.engine.load_precompiled(&k.dir.join(CODE_CACHE)) }.is_err()
            })
            .map(|(n, _)| n.clone())
            .collect();
        for name in stale {
            self.recompile(&name);
        }
    }

    /// Queues the compilation of an installed app's component.
    fn recompile(&mut self, name: &str) {
        let Some(known) = self.known.get(name) else {
            return;
        };
        let queued = self
            .jobs
            .iter()
            .chain(&self.compiling)
            .any(|(j, _)| matches!(j, Job::Recompile { name: n, .. } if n == name));
        if queued {
            return;
        }
        log::info!("app {name}: its compiled code is not this firmware's; compiling it again");
        let job = Job::Recompile {
            name: name.to_string(),
            wasm: known.dir.join(&known.manifest.wasm),
        };
        self.jobs.push_back((job, None));
        self.next_job();
    }

    /// True while an app is being compiled or waits to be.
    pub fn compiling(&self) -> bool {
        self.compiling.is_some() || !self.jobs.is_empty()
    }

    /// Hands the next waiting compilation to a worker thread.
    fn next_job(&mut self) {
        if self.compiling.is_some() {
            return;
        }
        let Some((job, responder)) = self.jobs.pop_front() else {
            return;
        };
        let inbox = self.inbox.clone();
        let wake = self.waker();
        let done = move |outcome| {
            inbox.push(Inbound::Compiled(outcome));
            wake();
        };
        match prepare::spawn(
            self.engine.clone(),
            self.installer.clone(),
            job.clone(),
            done,
        ) {
            Ok(()) => self.compiling = Some((job, responder)),
            Err(e) => {
                log::error!("apps: cannot start the compiler thread: {e}");
                if let Some(r) = responder {
                    r.reply(Err(CommandError::Failed(format!(
                        "cannot start the compiler thread: {e}"
                    ))));
                }
                self.next_job();
            }
        }
    }

    /// The worker thread finished the compilation in `self.compiling`.
    fn on_compiled(&mut self, outcome: prepare::Outcome) {
        let Some((job, responder)) = self.compiling.take() else {
            return;
        };
        match (job, outcome) {
            (Job::Install { .. }, prepare::Outcome::Install(result)) => {
                let result = result.and_then(|p| self.finish_install(*p));
                if let Err(e) = &result {
                    log::warn!("apps: an app was not installed: {e}");
                }
                if let Some(r) = responder {
                    r.reply(result.map(Reply::Text).map_err(|e| {
                        CommandError::Failed(format!("{e} (status {})", e.stock_status()))
                    }));
                }
            }
            (Job::Recompile { name, .. }, prepare::Outcome::Recompile(result)) => {
                self.finish_recompile(&name, result)
            }
            _ => {}
        }
        self.next_job();
    }

    /// Keeps the code compiled again for an installed app, and starts the
    /// app if it is one that runs without being opened.
    fn finish_recompile(&mut self, name: &str, result: Result<Vec<u8>, String>) {
        let Some(known) = self.known.get(name).filter(|k| !k.builtin) else {
            return;
        };
        let code = match result {
            Ok(code) => code,
            Err(e) => {
                log::warn!("app {name}: cannot be compiled: {e}");
                return;
            }
        };
        if let Err(e) = prepare::write_code(&known.dir, &code) {
            log::warn!("app {name}: cannot keep its compiled code: {e}");
            return;
        }
        if known.manifest.kind == AppType::Background || known.manifest.has_idle {
            if let Err(e) = self.start(name) {
                log::warn!("app {name}: could not start: {e}");
            }
            self.publish();
        }
    }

    fn publish(&mut self) {
        let (installed, disabled) = self
            .known
            .values()
            .map(|k| AppInfo {
                name: k.manifest.name.clone(),
                id: k.manifest.id.clone(),
                display_name: k.manifest.display_name.clone(),
                description: k.manifest.description.clone(),
                kind: match k.manifest.kind {
                    AppType::Foreground => opendeskphone_state::AppKind::Foreground,
                    AppType::Background => opendeskphone_state::AppKind::Background,
                },
                has_idle: k.manifest.has_idle,
                hidden: k.manifest.hidden,
                builtin: k.builtin,
                icon: k.manifest.icon.clone(),
                icon_png: opendeskphone_state::Picture(
                    k.manifest.icon_for(self.store.model().name()).into(),
                ),
                running: self.running.contains_key(&k.manifest.name),
            })
            .partition(|a| !a.builtin || self.services.config.app_enabled(&a.name));
        let apps = Apps {
            installed,
            disabled,
            foreground: self.foreground.clone(),
            idle_app: self.idle_shown.iter().min().cloned(),
        };
        if *self.store.get::<Apps>() != apps {
            self.store.set(apps);
        }
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.running.contains_key(name)
    }

    pub fn foreground(&self) -> Option<&str> {
        self.foreground.as_deref()
    }

    pub fn names(&self) -> Vec<String> {
        self.known.keys().cloned().collect()
    }

    fn load_code(&mut self, name: &str) -> Result<Component, String> {
        let known = self.known.get(name).ok_or("no such app")?;
        if let Some(c) = &known.code {
            return Ok(c.clone());
        }
        let cached = if known.builtin {
            known.dir.join(format!("{}.cwasm", known.manifest.name))
        } else {
            known.dir.join(CODE_CACHE)
        };
        // SAFETY: built-in code comes from the firmware image, and the
        // cache of an installed app was written by this manager (a
        // package cannot supply a dot file or a `.cwasm`).
        let loaded = unsafe { self.engine.load_precompiled(&cached) };
        let builtin = known.builtin;
        let component = match loaded {
            Ok(c) => c,
            // A built-in app staged with its component (a development
            // tree; the image has compiled code only).
            Err(_) if builtin && known.dir.join(&known.manifest.wasm).is_file() => {
                let wasm = std::fs::read(known.dir.join(&known.manifest.wasm))
                    .map_err(|e| format!("reading {}: {e}", known.manifest.wasm))?;
                self.engine.load_wasm(&wasm).map_err(|e| e.to_string())?
            }
            // An installed app whose code another firmware compiled: the
            // worker thread compiles it again, which takes seconds on the
            // phone, too long for this thread.
            Err(_) if !builtin && AppEngine::can_compile() => {
                self.recompile(name);
                return Err(COMPILING.into());
            }
            Err(e) => return Err(format!("compiled code does not load: {e}")),
        };
        if let Some(k) = self.known.get_mut(name) {
            k.code = Some(component.clone());
        }
        Ok(component)
    }

    fn start(&mut self, name: &str) -> Result<(), String> {
        if self.running.contains_key(name) {
            return Ok(());
        }
        let component = self.load_code(name)?;
        let known = self.known.get(name).ok_or("no such app")?;
        let manifest = known.manifest.clone();
        let mut linker = Linker::<AppCtx>::new(self.engine.wasmtime());
        host::link(&mut linker, &manifest).map_err(|e| format!("{e:#}"))?;

        let wake = self.waker();
        let window = self.shell.open(name, WindowKind::App, wake.clone());
        let idle = manifest
            .has_idle
            .then(|| self.shell.open(name, WindowKind::Idle, wake.clone()));
        let cap = match manifest.memory_kib as usize * 1024 {
            0 => self.cfg.memory_cap,
            want => want.min(self.cfg.memory_hard_cap),
        };
        self.generation += 1;
        let ctx = AppCtx {
            name: name.to_string(),
            generation: self.generation,
            local: crate::localstore::LocalStore::new(self.cfg.nv_root.join(name)),
            window,
            idle,
            idle_size: self.shell.idle_size(),
            store: self.store.clone(),
            services: self.services.clone(),
            inbox: self.inbox.clone(),
            actions: Vec::new(),
            timers: Vec::new(),
            next_timer: 0,
            next_request: 0,
            pending: HashSet::new(),
            observed: HashSet::new(),
            observe_calls: false,
            store_watch: false,
            presence_observed: HashSet::new(),
            hotdesk_pending: false,
            exit_after_background: true,
            foreground: false,
            pictures: HashMap::new(),
            images: HashSet::new(),
            playing: false,
            preview_until_ms: 0,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(cap)
                .memories(1)
                .tables(4)
                .table_elements(50_000)
                .instances(8)
                .build(),
            budget: Budget::new(self.cfg.start_budget, self.cfg.whole_budget),
            manifest,
        };
        let mut store = wasmtime::Store::new(self.engine.wasmtime(), ctx);
        store.limiter(|c| &mut c.limits);
        store.epoch_deadline_callback(|s| at_deadline(&s.data().budget));
        arm(&mut store, self.cfg.start_budget, self.cfg.whole_budget);
        hold_changes(store.data_mut(), true);
        let started = App::instantiate(&mut store, &component, &linker).and_then(|app| {
            app.opendeskphone_app_guest().call_start(&mut store)?;
            store.data().budget.in_time()?;
            Ok(app)
        });
        match started {
            Ok(app) => {
                hold_changes(store.data_mut(), false);
                let started_ms = opendeskphone_state::clock::mono_ms();
                self.running
                    .insert(name.to_string(), Running { store, app, started_ms });
                log::info!("app {name}: started");
                // What the app asked for in `start` may be for another
                // app, which sees who is running in the published list.
                self.publish();
                self.after_call(name);
                Ok(())
            }
            Err(e) => {
                let has_idle = store.data().idle.is_some();
                drop(store);
                self.shell.close(name, WindowKind::App);
                if has_idle {
                    self.shell.close(name, WindowKind::Idle);
                }
                self.faulted(name, None);
                Err(format!("{e:#}"))
            }
        }
    }

    /// Whether the app is one that runs without being opened, and the
    /// configuration lets it.
    fn starts_with_phone(&self, name: &str) -> bool {
        self.known.get(name).is_some_and(|k| {
            (k.manifest.kind == AppType::Background || k.manifest.has_idle)
                && (!k.builtin || self.services.config.app_enabled(name))
        })
    }

    /// An instance of the app faulted, after running since `started_ms`
    /// (`None`: it did not get through `start`).  An app that starts with
    /// the phone is started again later: after `restart_delay`, and after
    /// twice as long for each further fault in a row.
    fn faulted(&mut self, name: &str, started_ms: Option<u64>) {
        if !self.starts_with_phone(name) {
            self.faults.remove(name);
            return;
        }
        let now = opendeskphone_state::clock::mono_ms();
        let forgiven = self.cfg.restart_forgiven.as_millis() as u64;
        let faults = self.faults.entry(name.to_string()).or_default();
        if started_ms.is_some_and(|t| now.saturating_sub(t) >= forgiven) {
            faults.count = 0;
        }
        faults.count = faults.count.saturating_add(1);
        let delay = restart_delay(&self.cfg, faults.count);
        faults.restart_at_ms = Some(now + delay.as_millis() as u64);
        log::warn!(
            "app {name}: fault {} in a row; starting it again in {} s",
            faults.count,
            delay.as_secs()
        );
    }

    /// Starts the faulted apps whose time has come.
    fn restart_faulted(&mut self) {
        let now = opendeskphone_state::clock::mono_ms();
        let due: Vec<String> = self
            .faults
            .iter()
            .filter(|(_, f)| f.restart_at_ms.is_some_and(|t| t <= now))
            .map(|(n, _)| n.clone())
            .collect();
        for name in due {
            if let Some(f) = self.faults.get_mut(&name) {
                f.restart_at_ms = None;
            }
            if !self.starts_with_phone(&name) {
                // Removed, or switched off, meanwhile.
                self.faults.remove(&name);
                continue;
            }
            if self.running.contains_key(&name) {
                continue;
            }
            log::info!("app {name}: starting it again after a fault");
            match self.start(&name) {
                Ok(()) => {}
                Err(e) if e == COMPILING => {}
                // `start` has counted the fault and set the next time.
                Err(e) => log::warn!("app {name}: could not start: {e}"),
            }
            self.publish();
        }
    }

    /// Opens an app on screen, starting it if needed.
    pub fn launch(&mut self, name: &str) -> Result<(), String> {
        let known = self
            .known
            .get(name)
            .ok_or_else(|| format!("no app named {name:?}"))?;
        if known.builtin && !self.services.config.app_enabled(name) {
            return Err(format!("the app {name:?} is disabled"));
        }
        // Opened by hand: the faults before do not count against the app,
        // and there is no restart left to wait for.
        self.faults.remove(name);
        self.start(name)?;
        self.shell.show(name);
        self.publish();
        Ok(())
    }

    /// Stops an app: delivers `exit`, drops the instance, closes windows.
    pub fn close(&mut self, name: &str) {
        if self.running.contains_key(name) {
            self.call(name, &wit::Event::Exit);
        }
        self.kill(name);
        // Stopped on purpose: a restart that waits is not wanted, nor one
        // for a fault on the way out.
        if let Some(f) = self.faults.get_mut(name) {
            f.restart_at_ms = None;
        }
        self.publish();
    }

    /// Ends the calls that belong to the app `name` (hidden calls whose
    /// push the app got) when the app leaves the screen or stops, as
    /// stock does when its app screen unloads: such a call has no other
    /// screen from which to end it.  A call that several apps got ends
    /// with the last of them, and a call that no app got (none observed
    /// the call's event) ends when the app on screen leaves
    /// (`was_foreground`).
    fn end_hidden_calls(&mut self, name: &str, was_foreground: bool) {
        let calls = self.store.get::<opendeskphone_state::Calls>();
        for call in calls.live().filter(|c| c.hidden) {
            let end = match self.hidden_calls.get_mut(&call.id.0) {
                Some(owners) => {
                    let was_owner = owners.iter().any(|o| o == name);
                    owners.retain(|o| o != name);
                    was_owner && owners.is_empty()
                }
                None => was_foreground,
            };
            if end {
                self.hidden_calls.remove(&call.id.0);
                let _ = self.store.post(Command::Hangup { call: call.id });
            }
        }
    }

    fn kill(&mut self, name: &str) {
        self.end_hidden_calls(name, self.foreground.as_deref() == Some(name));
        if let Some(r) = self.running.remove(name) {
            let has_idle = r.store.data().idle.is_some();
            let c = r.store.data();
            if c.playing || opendeskphone_state::clock::mono_ms() < c.preview_until_ms {
                // Its sound does not outlive it.
                let _ = self.store.post(Command::StopSound);
            }
            drop(r);
            self.shell.hide(name);
            self.shell.close(name, WindowKind::App);
            if has_idle {
                self.shell.close(name, WindowKind::Idle);
            }
            if self.foreground.as_deref() == Some(name) {
                self.foreground = None;
            }
            self.idle_shown.remove(name);
            log::info!("app {name}: stopped");
            self.sync_store_watch();
        }
    }

    /// Tells the PBX's store which apps watch it, when that has changed.
    fn sync_store_watch(&mut self) {
        let watching =
            self.apps_where(|c| c.store_watch && c.has(Capability::Storage));
        if watching != self.store_watch {
            self.services.store.watch(&watching);
            self.store_watch = watching;
        }
    }

    /// The PBX's store came within reach, or a key of an app changed
    /// there.
    fn on_store_signal(&mut self, signal: crate::services::StoreSignal) {
        use crate::services::StoreSignal;
        match signal {
            StoreSignal::Online => {
                for name in self.apps_where(|c| c.has(Capability::Storage)) {
                    self.deliver(&name, wit::Event::StorageOnline);
                }
            }
            StoreSignal::Changed { app, scope, key } => {
                let watching = self
                    .running
                    .get(&app)
                    .is_some_and(|r| r.store.data().store_watch);
                if watching {
                    let change = wit::StorageChange {
                        location: host::location(scope),
                        key,
                    };
                    self.deliver(&app, wit::Event::StorageChanged(change));
                }
            }
        }
    }

    /// Calls the app's event handler.  A trap (including running over a
    /// time budget or the memory cap) stops the app.
    fn call(&mut self, name: &str, event: &wit::Event) -> bool {
        let (budget, whole) = (self.cfg.call_budget, self.cfg.whole_budget);
        let Some(r) = self.running.get_mut(name) else {
            return false;
        };
        arm(&mut r.store, budget, whole);
        // What the handler changes in the app's windows is shown when the
        // handler has returned, together.
        hold_changes(r.store.data_mut(), true);
        let result = r
            .app
            .opendeskphone_app_guest()
            .call_handle(&mut r.store, event)
            .and_then(|()| r.store.data().budget.in_time());
        hold_changes(r.store.data_mut(), false);
        match result {
            Ok(()) => true,
            Err(e) => {
                log::error!("app {name}: stopped after a fault: {e:#}");
                let started_ms = r.started_ms;
                self.kill(name);
                self.faulted(name, Some(started_ms));
                self.publish();
                false
            }
        }
    }

    /// Tells the app that called `hotdesk.login` or `logout` how it
    /// ended, once the provisioning state says.
    fn on_user_selected(&mut self) {
        let waiting = self.apps_where(|c| c.hotdesk_pending);
        if waiting.is_empty() {
            return;
        }
        let Some(result) = self.store.get::<opendeskphone_state::Provisioning>().selection.clone() else {
            return;
        };
        for name in waiting {
            if let Some(r) = self.running.get_mut(&name) {
                r.store.data_mut().hotdesk_pending = false;
            }
            self.deliver(&name, wit::Event::HotdeskDone(result.clone()));
        }
    }

    /// Delivers an event and applies what the app asked for meanwhile.
    fn deliver(&mut self, name: &str, event: wit::Event) {
        if self.call(name, &event) {
            self.after_call(name);
        }
    }

    fn after_call(&mut self, name: &str) {
        let actions = match self.running.get_mut(name) {
            Some(r) => std::mem::take(&mut r.store.data_mut().actions),
            None => return,
        };
        for a in actions {
            match a {
                Action::Background => self.shell.hide(name),
                Action::Exit => {
                    self.close(name);
                    return;
                }
                Action::Restart => {
                    let was_fg = self.foreground.as_deref() == Some(name);
                    self.close(name);
                    let r = if was_fg {
                        self.launch(name)
                    } else {
                        self.start(name)
                    };
                    if let Err(e) = r {
                        log::warn!("app {name}: restart failed: {e}");
                    }
                    return;
                }
                Action::ShowApp(other) => {
                    if let Err(e) = self.launch(&other) {
                        log::warn!("app {name}: cannot show {other}: {e}");
                    }
                }
                Action::Send {
                    app,
                    name: ev,
                    payload,
                } => {
                    if self.running.contains_key(&app) {
                        self.deliver(&app, wit::Event::Message((ev, payload)));
                    }
                }
                Action::StoreWatch => self.sync_store_watch(),
            }
        }
    }

    /// Installs an app file.  Returns the app's name.
    ///
    /// This compiles on the calling thread, which takes seconds on the
    /// phone; the command `InstallApp` has the worker thread do it.
    pub fn install(
        &mut self,
        path: &Path,
        required_name: Option<&str>,
    ) -> Result<String, InstallError> {
        let prepared = prepare::prepare(&self.engine, &self.installer, path, required_name)?;
        self.finish_install(prepared)
    }

    /// Puts a checked and compiled app in place.
    fn finish_install(&mut self, prepared: Prepared) -> Result<String, InstallError> {
        let Prepared { package, code } = prepared;
        let name = package.manifest.name.clone();
        // Again, now with the compiled code's size, and because the
        // compilation took a while.
        self.installer.check(&package, None, code.len() as u64)?;
        let was_running = self.running.contains_key(&name);
        self.close(&name);
        self.installer
            .commit(&package, &[(CODE_CACHE.to_string(), code)])?;
        self.scan();
        self.store.emit(Event::App {
            app: name.clone(),
            name: "app_install".into(),
            payload: String::new(),
        });
        if was_running {
            let _ = self.start(&name);
            self.publish();
        }
        log::info!("app {name}: installed");
        Ok(name)
    }

    /// Removes an installed app and its data.
    pub fn remove(&mut self, name: &str) -> Result<(), InstallError> {
        if self.known.get(name).is_some_and(|k| k.builtin) {
            return Err(InstallError::ShadowsBuiltin(name.to_string()));
        }
        self.close(name);
        self.installer.remove(name)?;
        self.scan();
        Ok(())
    }

    fn handle_command(&mut self, command: Command) -> Result<Reply, CommandError> {
        match command {
            Command::LaunchApp { name } => self
                .launch(&name)
                .map(|_| Reply::Done)
                .map_err(CommandError::Failed),
            Command::CloseApp { name } => {
                self.close(&name);
                Ok(Reply::Done)
            }
            Command::RemoveApp { name } => self
                .remove(&name)
                .map(|_| Reply::Done)
                .map_err(|e| CommandError::Failed(e.to_string())),
            other => Err(CommandError::Invalid(format!(
                "not an app command: {other:?}"
            ))),
        }
    }

    /// Follows the configuration's choice of built-in apps: stops the ones
    /// it switches off, and starts the ones it switches back on that run
    /// without being opened.
    fn follow_enabled(&mut self) {
        for name in self.running_names() {
            if self.known.get(&name).is_some_and(|k| k.builtin)
                && !self.services.config.app_enabled(&name)
            {
                log::info!("app {name}: switched off by the configuration");
                self.close(&name);
            }
        }
        let back_on: Vec<String> = self
            .store
            .get::<Apps>()
            .disabled
            .iter()
            .filter(|a| a.kind == opendeskphone_state::AppKind::Background || a.has_idle)
            .filter(|a| self.services.config.app_enabled(&a.name))
            .map(|a| a.name.clone())
            .collect();
        for name in back_on {
            if let Err(e) = self.start(&name) {
                log::warn!("app {name}: could not start: {e}");
            }
        }
        self.publish();
    }

    fn running_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.running.keys().cloned().collect();
        v.sort();
        v
    }

    fn apps_where(&self, f: impl Fn(&AppCtx) -> bool) -> Vec<String> {
        let mut v: Vec<String> = self
            .running
            .iter()
            .filter(|(_, r)| f(r.store.data()))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    fn on_state(&mut self, wake: opendeskphone_state::Wake) {
        if wake.changed(Topic::Calls) {
            // What an app sees of a call, but for the duration, which
            // counts on without a change.
            fn seen(c: &wit::CallInfo) -> (u32, u8, bool, wit::CallState, &str, &str) {
                (c.id, c.account, c.incoming, c.state, &c.remote_name, &c.remote_number)
            }
            let calls: Vec<wit::CallInfo> =
                self.store.get::<opendeskphone_state::Calls>().calls.iter().map(host::call_info).collect();
            // The end of a call comes with `Event::CallEnded`, once.
            let changed: Vec<wit::CallInfo> = calls
                .iter()
                .filter(|c| c.state != wit::CallState::Ended)
                .filter(|c| self.last_calls.iter().find(|o| o.id == c.id).map(seen) != Some(seen(c)))
                .cloned()
                .collect();
            self.hidden_calls.retain(|id, _| calls.iter().any(|c| c.id == *id));
            self.last_calls = calls;
            if !changed.is_empty() {
                for name in self.apps_where(|c| c.observe_calls && c.has(Capability::Calls)) {
                    for info in &changed {
                        self.deliver(&name, wit::Event::Call(info.clone()));
                    }
                }
            }
        }
        if wake.changed(Topic::Mwi) {
            let mwi = self.store.get::<opendeskphone_state::Mwi>();
            let changed: Vec<wit::Mwi> = mwi
                .accounts
                .iter()
                .filter(|m| !self.last_mwi.accounts.contains(m))
                .map(|m| wit::Mwi {
                    account: m.account.0,
                    new_messages: m.new,
                    old_messages: m.old,
                })
                .collect();
            self.last_mwi = mwi;
            for name in self.apps_where(|c| c.has(Capability::Calls)) {
                for m in &changed {
                    self.deliver(&name, wit::Event::Mwi(*m));
                }
            }
        }
        if wake.changed(Topic::Presence) {
            let now = self.store.get::<opendeskphone_state::Presence>();
            let changed: Vec<opendeskphone_state::BlfEntry> = now
                .entries
                .iter()
                .filter(|e| self.last_presence.get(&e.id) != Some(e))
                .cloned()
                .collect();
            self.last_presence = now;
            for name in self.apps_where(|c| !c.presence_observed.is_empty()) {
                let subs: Vec<String> = self.running[&name]
                    .store
                    .data()
                    .presence_observed
                    .iter()
                    .cloned()
                    .collect();
                for e in &changed {
                    for t in subs.iter().filter(|t| host::entry_matches(e, t)) {
                        self.deliver(&name, wit::Event::Presence(host::presence_of(e, t)));
                    }
                }
            }
        }
        if wake.changed(Topic::Provisioning) {
            self.on_user_selected();
        }
        if wake.changed(Topic::Alarms) {
            for name in self.apps_where(|c| c.has(Capability::Alarms)) {
                self.deliver(&name, wit::Event::AlarmsChanged);
            }
        }
        for ev in wake.events {
            match ev {
                Event::Push { name: push, body } => {
                    let record = wit::Push {
                        name: push,
                        kind: String::new(),
                        data: body,
                        method: wit::PushMethod::Message,
                        call: None,
                    };
                    self.on_push(record)
                }
                Event::CallPush { name: push, kind, data, method, call } => {
                    let method = host::push_method(&method);
                    let record = wit::Push { name: push, kind, data, method, call: Some(call.0) };
                    self.on_push(record)
                }
                Event::App {
                    app,
                    name: ev,
                    payload,
                } => {
                    if ev != "app_install" && self.running.contains_key(&app) {
                        self.deliver(&app, wit::Event::Message((ev, payload)));
                    }
                }
                Event::PlaybackStopped { app, position_secs } => {
                    if let Some(r) = self.running.get_mut(&app) {
                        r.store.data_mut().playing = false;
                        self.deliver(&app, wit::Event::PlaybackStopped(position_secs));
                    }
                }
                Event::CallEnded { call } => {
                    let info = host::call_info(&call);
                    for name in self.apps_where(|c| c.observe_calls && c.has(Capability::Calls)) {
                        self.deliver(&name, wit::Event::Call(info.clone()));
                    }
                }
                Event::ConfigApplied { .. } => {
                    self.follow_enabled();
                    for name in self.apps_where(|c| c.has(Capability::Config)) {
                        self.deliver(&name, wit::Event::NewConfig);
                    }
                }
                Event::ShuttingDown { .. } => {
                    for name in self.running_names() {
                        self.close(&name);
                    }
                }
                _ => {}
            }
        }
    }

    /// A server push, for the apps that observe its name.
    fn on_push(&mut self, mut record: wit::Push) {
        let push = record.name.clone();
        if push == "digium.phone.server_change" {
            for name in self.apps_where(|c| c.has(Capability::Events)) {
                self.deliver(&name, wit::Event::ServerChange);
            }
            return;
        }
        let targets = self.apps_where(|c| c.observed.contains(&push));
        if targets.is_empty() {
            return;
        }
        // An INVITE push names its call; if the SIP layer did not, take the
        // newest alerting call.
        let calls = self.store.get::<opendeskphone_state::Calls>();
        if record.call.is_none() && push.starts_with("digium.incomingCall.") {
            record.method = wit::PushMethod::Invite;
            record.call = calls
                .calls
                .iter()
                .filter(|c| c.state == opendeskphone_state::CallState::Ringing)
                .max_by_key(|c| c.started_mono_ms)
                .map(|c| c.id.0);
        }
        // A hidden call is the call of the apps that get the call's push.
        if let Some(id) = record.call
            && calls.live().any(|c| c.hidden && c.id.0 == id)
        {
            let owners = self.hidden_calls.entry(id).or_default();
            for name in &targets {
                if !owners.contains(name) {
                    owners.push(name.clone());
                }
            }
        }
        for name in targets {
            self.deliver(&name, wit::Event::Push(record.clone()));
        }
    }

    fn on_window_event(&mut self, name: &str, idle: bool, ev: WindowEvent) {
        let bit = if idle { IDLE_BIT } else { 0 };
        let event = match ev {
            WindowEvent::Foreground if idle => {
                self.idle_shown.insert(name.to_string());
                self.publish();
                wit::Event::IdleScreenShow
            }
            WindowEvent::Background if idle => {
                self.idle_shown.remove(name);
                self.publish();
                wit::Event::IdleScreenHide
            }
            WindowEvent::Foreground => {
                self.foreground = Some(name.to_string());
                if let Some(r) = self.running.get_mut(name) {
                    r.store.data_mut().foreground = true;
                }
                self.publish();
                wit::Event::Foreground
            }
            WindowEvent::Background => {
                let was_foreground = self.foreground.as_deref() == Some(name);
                if was_foreground {
                    self.foreground = None;
                }
                self.end_hidden_calls(name, was_foreground);
                let exit = match self.running.get_mut(name) {
                    Some(r) => {
                        let c = r.store.data_mut();
                        c.foreground = false;
                        c.exit_after_background
                            && c.manifest.kind == AppType::Foreground
                            && !c.manifest.has_idle
                    }
                    None => false,
                };
                self.deliver(name, wit::Event::Background);
                if exit && self.foreground.as_deref() != Some(name) {
                    self.close(name);
                }
                self.publish();
                return;
            }
            WindowEvent::Key(k) => wit::Event::Key(host::wit_key(k)),
            WindowEvent::Hook(off_hook) => wit::Event::Hook(off_hook),
            WindowEvent::Softkey(id) => wit::Event::Softkey(id),
            WindowEvent::Highlighted { widget, row } => {
                wit::Event::Widget((widget | bit, wit::WidgetEvent::Highlighted(row)))
            }
            WindowEvent::Activated { widget, row } => {
                wit::Event::Widget((widget | bit, wit::WidgetEvent::Activated(row)))
            }
            WindowEvent::Changed { widget, value } => {
                wit::Event::Widget((widget | bit, wit::WidgetEvent::Changed(value)))
            }
            WindowEvent::Submitted { widget, value } => {
                wit::Event::Widget((widget | bit, wit::WidgetEvent::Submitted(value)))
            }
            WindowEvent::Touch { widget, touch } => wit::Event::Touch((widget | bit, touch)),
            WindowEvent::CloseRequested => {
                self.shell.hide(name);
                return;
            }
        };
        self.deliver(name, event);
    }

    /// Does all pending work.  Returns how long until a timer is due or
    /// a faulted app is to start again, if there is either.
    pub fn pump(&mut self) -> Option<Duration> {
        let waker = Waker::from(self.inbox.notifier.clone());
        let mut cx = Context::from_waker(&waker);

        while let Poll::Ready(Some(envelope)) = self.commands.poll_next(&mut cx) {
            let (command, responder) = envelope.split();
            if let Command::InstallApp { path } = command {
                // Answered when the worker thread has compiled the app.
                let job = Job::Install { path: path.into() };
                self.jobs.push_back((job, Some(responder)));
                self.next_job();
                continue;
            }
            let result = self.handle_command(command);
            responder.reply(result);
        }

        loop {
            let item = self.inbox.queue.lock().unwrap().pop_front();
            let Some(item) = item else { break };
            let (app, generation, id, event) = match item {
                Inbound::Compiled(outcome) => {
                    self.on_compiled(outcome);
                    continue;
                }
                Inbound::StoreSignal(signal) => {
                    self.on_store_signal(signal);
                    continue;
                }
                Inbound::Store {
                    app,
                    generation,
                    id,
                    outcome,
                } => (app, generation, id, wit::Event::Storage((id, outcome))),
                Inbound::Pbx {
                    app,
                    generation,
                    id,
                    outcome,
                } => (app, generation, id, wit::Event::Pbx((id, outcome))),
                Inbound::Http {
                    app,
                    generation,
                    id,
                    result,
                } => (app, generation, id, wit::Event::Http((id, result))),
            };
            let live = self.running.get_mut(&app).is_some_and(|r| {
                let c = r.store.data_mut();
                c.generation == generation && c.pending.remove(&id)
            });
            if live {
                self.deliver(&app, event);
            }
        }

        while let Poll::Ready(wake) = self.sub.poll_changed(&mut cx) {
            self.on_state(wake);
        }

        // Window events; bounded so a chatty window cannot starve the rest.
        for _ in 0..64 {
            let mut any = false;
            for name in self.running_names() {
                let (ev, idle) = match self.running.get_mut(&name) {
                    Some(r) => {
                        let c = r.store.data_mut();
                        match c.window.poll_event() {
                            Some(e) => (Some(e), false),
                            None => (c.idle.as_mut().and_then(|w| w.poll_event()), true),
                        }
                    }
                    None => (None, false),
                };
                if let Some(ev) = ev {
                    any = true;
                    self.on_window_event(&name, idle, ev);
                }
            }
            if !any {
                break;
            }
        }

        let now = opendeskphone_state::clock::mono_ms();
        for name in self.running_names() {
            let due: Vec<u32> = match self.running.get_mut(&name) {
                Some(r) => {
                    let c = r.store.data_mut();
                    let due: Vec<u32> = c
                        .timers
                        .iter()
                        .filter(|t| t.deadline_ms <= now)
                        .map(|t| t.id)
                        .collect();
                    c.timers.retain_mut(|t: &mut Timer| {
                        if t.deadline_ms > now {
                            return true;
                        }
                        match t.repeat_ms {
                            Some(p) => {
                                t.deadline_ms = now + p as u64;
                                true
                            }
                            None => false,
                        }
                    });
                    due
                }
                None => Vec::new(),
            };
            for id in due {
                self.deliver(&name, wit::Event::Timer(id));
            }
        }

        self.restart_faulted();

        self.store.heartbeat(Subsystem::Apps);
        let now = opendeskphone_state::clock::mono_ms();
        self.running
            .values()
            .flat_map(|r| r.store.data().timers.iter().map(|t| t.deadline_ms))
            .chain(self.faults.values().filter_map(|f| f.restart_at_ms))
            .min()
            .map(|d| Duration::from_millis(d.saturating_sub(now)))
    }

    /// Runs until the stop flag is set.
    pub fn run(mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            let next = self.pump().unwrap_or(Duration::from_secs(1));
            self.inbox.notifier.wait(next.min(Duration::from_secs(1)));
        }
        for name in self.running_names() {
            self.close(&name);
        }
    }
}

/// Holds back, or lets go, what is changed in the app's windows.
fn hold_changes(c: &mut AppCtx, on: bool) {
    c.window.hold_changes(on);
    if let Some(idle) = c.idle.as_mut() {
        idle.hold_changes(on);
    }
}

/// Gives the guest call that follows `limit` of the app's code running,
/// and `whole` in all.
fn arm(store: &mut wasmtime::Store<AppCtx>, limit: Duration, whole: Duration) {
    store.data_mut().budget = Budget::new(limit, whole);
    store.set_epoch_deadline(ticks(limit.min(whole)));
}

/// What happens when a guest call reaches the epoch deadline: the app
/// stops if the app's code has run for the whole limit, or the call has
/// taken all of `whole`.  Otherwise (after a wait for the phone's answer,
/// for one) the deadline moves to when the first of the two runs out.
fn at_deadline(budget: &Budget) -> wasmtime::Result<UpdateDeadline> {
    budget.in_time()?;
    Ok(match budget.left() {
        left if left.is_zero() => UpdateDeadline::Interrupt,
        left => UpdateDeadline::Continue(ticks(left)),
    })
}

/// Epoch ticks in `d`, rounded up, and at least one.
fn ticks(d: Duration) -> u64 {
    (d.as_millis() as u64)
        .div_ceil(EPOCH_TICK.as_millis() as u64)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restart_delay_doubles_from_ten_seconds_to_an_hour() {
        let cfg = ManagerConfig::phone();
        let secs = |count| restart_delay(&cfg, count).as_secs();
        assert_eq!([secs(1), secs(2), secs(3), secs(4)], [10, 20, 40, 80]);
        // 10 * 2^8 is 2,560 seconds; the next would be over the hour.
        assert_eq!([secs(9), secs(10), secs(11), secs(40), secs(u32::MAX)], [2560, 3600, 3600, 3600, 3600]);
    }
}
