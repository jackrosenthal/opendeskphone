//! Shared fixture: the built-in apps and the test guest, built to
//! WebAssembly and compiled for the host interpreter once per test run.
//!
//! With the feature `test-inputs/required` (the Nix check, whose sandbox
//! cannot build them), they are `test-inputs/apps-staged` instead, staged in
//! an earlier step (see the `test-inputs` crate).

#![allow(dead_code)]

use opendeskphone_apps::engine::Target;
use opendeskphone_apps::testutil::{FakeConfig, FakePbx, FakeStore, FakeUi, services_with_store};
use opendeskphone_apps::{AppManager, ManagerConfig};
use opendeskphone_state::{Command, CommandResult, Domain, Model, Reply, Store};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex, OnceLock};

fn run(cmd: &mut Process) {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{:?} failed:\n{}\n{}",
        cmd,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The staged apps root.
pub fn staged() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if test_inputs::REQUIRED {
            return test_inputs::input("apps-staged").unwrap();
        }
        let tool = env!("CARGO_BIN_EXE_opendeskphone-apps-build");
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("apps-fixture");
        let out = work.join("staged");
        let _ = std::fs::remove_dir_all(&out);
        for src in [crate_dir.join("../../apps"), crate_dir.join("tests/guests")] {
            run(Process::new(tool)
                .env("CARGO_TARGET_DIR", work.join("guest-target"))
                // Do not let the outer cargo's settings leak into the wasm build.
                .env_remove("CARGO_ENCODED_RUSTFLAGS")
                .env_remove("RUSTFLAGS")
                .args(["build", "--target", "pulley64", "--apps"])
                .arg(&src)
                .arg("--out")
                .arg(&out));
        }
        out
    })
}

/// The phone's record of a staged app.
#[allow(dead_code)]
pub fn record(app: &str) -> opendeskphone_apps::Manifest {
    let bytes = std::fs::read(staged().join(app).join(opendeskphone_apps::manifest::RECORD)).unwrap();
    opendeskphone_apps::Manifest::from_record(&bytes).unwrap()
}

/// The interfaces of the phone a staged app uses: its component's imports.
#[allow(dead_code)]
pub fn capabilities(app: &str) -> Vec<&'static str> {
    record(app).capabilities.iter().map(|c| c.name()).collect()
}

pub struct Rig {
    pub mgr: AppManager,
    pub ui: FakeUi,
    pub pbx: FakePbx,
    /// The PBX's store of app data, as this phone reaches it.
    pub app_data: FakeStore,
    pub store: Store,
    pub commands: Arc<Mutex<Vec<Command>>>,
    pub nvdata: PathBuf,
    _dir: TempDir,
}

pub struct TempDir(pub PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn temp_dir(tag: &str) -> TempDir {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    TempDir(d)
}

/// Answers commands of a domain on a thread, recording them.
pub fn answer_commands(
    store: &Store,
    domain: Domain,
    log: Arc<Mutex<Vec<Command>>>,
    reply: impl Fn(&Command) -> CommandResult + Send + 'static,
) {
    let rx = store.handle(domain);
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            let (cmd, responder) = env.split();
            let r = reply(&cmd);
            log.lock().unwrap().push(cmd);
            responder.reply(r);
        }
    });
}

/// The commands in `log` (of [`answer_commands`]) once it holds `n`,
/// waiting up to ten seconds for them; what it holds then, for the test
/// to show, if they do not come.
///
/// A command the code under test posts (`Store::post`) is not waited for:
/// it is recorded on the thread of [`answer_commands`], some time after
/// the code that posted it has gone on, so a test that looks at once may
/// look too soon.  A command that is sent and waited for is recorded
/// before it is answered.  With `n` 0 there is nothing to wait for, and a
/// command that should not have come is given 30 ms to show.
pub fn recorded(log: &Mutex<Vec<Command>>, n: usize) -> Vec<Command> {
    recorded_of(log, n, |_| true)
}

/// [`recorded`], of the commands that `of` picks.
pub fn recorded_of(log: &Mutex<Vec<Command>>, n: usize, of: impl Fn(&Command) -> bool) -> Vec<Command> {
    use std::time::{Duration, Instant};
    if n == 0 {
        std::thread::sleep(Duration::from_millis(30));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got: Vec<Command> = log.lock().unwrap().iter().filter(|c| of(c)).cloned().collect();
        if got.len() >= n || Instant::now() >= deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The time the phone gives an app's code for an event (2 seconds), which
/// the rigs give too.  The time is measured on the clock, so a shorter one
/// would test how busy the machine that runs the tests is, not the app;
/// only the tests of the time limit itself ask for less
/// ([`Rig::with_budget`]).
pub fn phone_budget() -> std::time::Duration {
    ManagerConfig::phone().call_budget
}

impl Rig {
    pub fn new(config: FakeConfig) -> Rig {
        Rig::with_root(staged(), config, Model::D60)
    }

    pub fn with_root(root: &Path, config: FakeConfig, model: Model) -> Rig {
        Rig::with_budget(root, config, model, phone_budget())
    }

    /// A rig whose apps get `budget` for each event instead of
    /// [`phone_budget`].
    pub fn with_budget(
        root: &Path,
        config: FakeConfig,
        model: Model,
        budget: std::time::Duration,
    ) -> Rig {
        Rig::with_app_data(root, config, model, budget, FakeStore::default())
    }

    /// A phone whose PBX store of app data is `app_data`: with
    /// `FakeStore::phone` of another rig's, a second phone of the same
    /// PBX.
    pub fn on_pbx(app_data: FakeStore) -> Rig {
        Rig::with_app_data(staged(), FakeConfig::default(), Model::D60, phone_budget(), app_data)
    }

    /// A rig with other services than the fakes: `change` gets the
    /// rig's, and returns what the apps are to have.
    pub fn with_services(
        change: impl FnOnce(opendeskphone_apps::Services) -> opendeskphone_apps::Services,
    ) -> Rig {
        let config = FakeConfig::default();
        Rig::build(staged(), config, Model::D60, phone_budget(), FakeStore::default(), change, |_| {})
    }

    /// A rig whose manager has other settings than a test's usual ones:
    /// `edit` changes them.
    pub fn with_manager(edit: impl FnOnce(&mut ManagerConfig)) -> Rig {
        let config = FakeConfig::default();
        Rig::build(staged(), config, Model::D60, phone_budget(), FakeStore::default(), |s| s, edit)
    }

    pub fn with_app_data(
        root: &Path,
        config: FakeConfig,
        model: Model,
        budget: std::time::Duration,
        app_data: FakeStore,
    ) -> Rig {
        Rig::build(root, config, model, budget, app_data, |services| services, |_| {})
    }

    fn build(
        root: &Path,
        config: FakeConfig,
        model: Model,
        budget: std::time::Duration,
        app_data: FakeStore,
        change: impl FnOnce(opendeskphone_apps::Services) -> opendeskphone_apps::Services,
        edit: impl FnOnce(&mut ManagerConfig),
    ) -> Rig {
        let dir = temp_dir("rig");
        let store = Store::new(model);
        let ui = FakeUi::new(model);
        let pbx = FakePbx::default();
        let commands = Arc::new(Mutex::new(Vec::new()));
        answer_commands(&store, Domain::Calls, commands.clone(), |c| match c {
            Command::Dial { .. } => Ok(Reply::Call(opendeskphone_state::CallId(7))),
            Command::DialWith { .. } => Ok(Reply::Call(opendeskphone_state::CallId(8))),
            _ => Ok(Reply::Done),
        });
        answer_commands(&store, Domain::Provisioning, commands.clone(), |_| {
            Ok(Reply::Done)
        });
        let nvdata = dir.0.join("nvdata");
        let mut cfg = ManagerConfig::rooted(root, &nvdata);
        cfg.target = Target::Pulley64;
        cfg.call_budget = budget;
        edit(&mut cfg);
        let services = change(services_with_store(&pbx, config, &app_data));
        let mut mgr = AppManager::new(cfg, store.clone(), ui.shell(), services).expect("engine");
        mgr.scan();
        Rig {
            mgr,
            ui,
            pbx,
            app_data,
            store,
            commands,
            nvdata,
            _dir: dir,
        }
    }

    /// Runs the manager until nothing more happens.
    pub fn settle(&mut self) {
        for _ in 0..8 {
            self.mgr.pump();
        }
        // A compilation on the worker thread (an install, or an installed
        // app's code compiled again) is waited for.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while self.mgr.compiling() {
            assert!(std::time::Instant::now() < deadline, "the compiler thread did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.mgr.pump();
        }
        for _ in 0..8 {
            self.mgr.pump();
        }
    }

    pub fn launch(&mut self, app: &str) {
        self.mgr
            .launch(app)
            .unwrap_or_else(|e| panic!("launch {app}: {e}"));
        self.settle();
    }

    pub fn softkey(&mut self, app: &str, label: &str) {
        self.ui.press_softkey(app, label);
        self.settle();
    }

    pub fn key(&mut self, app: &str, key: opendeskphone_state::Key) {
        self.ui.press(app, key);
        self.settle();
    }

    pub fn commands(&self) -> Vec<Command> {
        self.commands.lock().unwrap().clone()
    }
}

// ------------------------------------------- touch: the SDK's controls

/// A button an app drew on the touchscreen (`controls::Button` of the
/// SDK): its label, where it is (in the widget it is in), and the color
/// of its inside.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub label: String,
    pub at: opendeskphone_apps::Rect,
    pub fill: opendeskphone_apps::Color,
}

/// The labeled buttons of the app's window, in the order the app made
/// them.
pub fn buttons(r: &Rig, app: &str) -> Vec<Drawn> {
    use opendeskphone_apps::DrawOp;
    r.ui.drawings(app)
        .into_iter()
        .filter_map(|(at, ops)| {
            let label = ops.iter().find_map(|op| match op {
                DrawOp::Text(t) => Some(t.text.clone()),
                _ => None,
            })?;
            let fill = ops.iter().rev().find_map(|op| match op {
                DrawOp::Fill((_, c)) => Some(c.clone()),
                _ => None,
            })?;
            Some(Drawn { label, at, fill })
        })
        .collect()
}

/// The labels of the app's buttons.
pub fn labels(r: &Rig, app: &str) -> Vec<String> {
    buttons(r, app).into_iter().map(|b| b.label).collect()
}

/// The button labeled `label`.  Panics when there is none.
pub fn button(r: &Rig, app: &str, label: &str) -> Drawn {
    let all = buttons(r, app);
    all.iter().find(|b| b.label == label).cloned().unwrap_or_else(|| {
        panic!("no button {label:?}; have {:?}", all.iter().map(|b| &b.label).collect::<Vec<_>>())
    })
}

/// A finger touches the middle of the button labeled `label`, which is in
/// the window (not in another widget).
pub fn press(r: &mut Rig, app: &str, label: &str) {
    let at = button(r, app, label).at;
    r.ui.tap(app, at.x as i32 + at.width as i32 / 2, at.y as i32 + at.height as i32 / 2);
    r.settle();
}

/// The colors the fake gives the skin's roles (`FakeUi`'s
/// `theme_color`): the selection's, for a button that is chosen or the
/// screen's main action.
pub fn highlight() -> opendeskphone_apps::Color {
    opendeskphone_apps::Color { r: 48, g: 48, b: 48 }
}

/// The red of a button that deletes.
pub fn red() -> opendeskphone_apps::Color {
    opendeskphone_apps::Color { r: 0xB7, g: 0x1C, b: 0x1C }
}

/// The green of a button that calls.
pub fn green() -> opendeskphone_apps::Color {
    opendeskphone_apps::Color { r: 0x38, g: 0x8E, b: 0x3C }
}
