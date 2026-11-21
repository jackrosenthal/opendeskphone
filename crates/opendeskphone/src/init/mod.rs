//! The init role: PID 1.
//!
//! It does only what must not fail (section 4 of the plan):
//!
//! 1. Mount `/proc`, `/sys`, `/dev`, `/dev/pts`, and tmpfs for `/tmp`, `/run`
//!    (and `/var` when the rootfs has it).
//! 2. Arm `/dev/watchdog` and feed it.
//! 3. Load the kernel modules for the detected model.
//! 4. Mount nvdata by name.  A failed mount never erases anything; a tmpfs
//!    stands in and the main role is told to run in recovery.
//! 5. Restore the clock from `/nvdata/systime`.
//! 6. Start the main role and supervise it.
//!
//! No async runtime, no threads, no allocation-heavy dependencies.  Errors
//! are logged to the kernel log and survived; nothing here unwraps.
//!
//! ## Supervision contract with the main role
//!
//! The main role inherits the write end of a pipe as file descriptor
//! [`HEARTBEAT_FD`] (also named by `dphone main --heartbeat-fd`).  It writes [`BEAT_ALIVE`] at least every few
//! seconds while its event loop turns, and [`BEAT_HEALTHY`] once it considers
//! the phone healthy (network up, UI running).  A main role in safe mode
//! writes [`BEAT_SAFE`] in place of [`BEAT_HEALTHY`]: it is up, but it is not
//! the phone, so it never confirms a trial boot.
//!
//! - No beat for [`START_GRACE_SECS`] after start, or for
//!   [`BEAT_TIMEOUT_SECS`] later: the main role is killed and restarted.
//! - Exit or kill: restart with exponential backoff up to
//!   [`MAX_BACKOFF_SECS`].  After [`SAFE_MODE_AFTER_CRASHES`] failures in a
//!   row the main role is started in safe mode, except during a trial boot:
//!   there the full main role is tried until
//!   [`trial::MAX_TRIAL_CRASHES`] failures roll the phone back.
//! - No beat from any main-role instance for [`UNHEALTHY_REBOOT_SECS`]: clean
//!   reboot, since a restart did not help.  Suppressed while
//!   `/run/opendeskphone/no-reboot` exists, so a serial-console user can keep
//!   working on a crash-looping phone.
//! - The hardware watchdog is fed from this loop only, so it covers a hung
//!   kernel or a hung init role; a hung main role is handled by the rules
//!   above rather than by starving the watchdog, which would skip the clean
//!   unmount of nvdata.
//! - Signals to PID 1: `SIGTERM` reboot, `SIGUSR1` halt, `SIGUSR2` power off,
//!   and `SIGRTMIN+6` to start the kernel that `dphone ramboot` loaded.
//!
//! ## A system started from RAM
//!
//! `dphone ramboot` starts an image's kernel with its root file system in
//! RAM.  Such a system is from neither firmware bank
//! ([`trial::started_from_ram`]), and the image a pending trial is about
//! is not the one running: the trial record is left as it is, neither
//! counted nor confirmed.  nvdata is mounted as always (the settings, the
//! address, and the SSH keys are there).  `dphone.max_uptime=` on the
//! command line ends the run by itself (see `Supervisor::periodic`).

pub mod modules;
pub mod mounts;
pub mod nvdata;
pub mod trial;
pub mod watchdog;

use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{error, info, warn};

use crate::model;
use crate::paths::Paths;
use crate::sys::{self, Shutdown};

use self::nvdata::NvdataState;
use self::trial::{RollbackHook, TrialRecord};
use self::watchdog::Watchdog;

/// What the init role takes from its environment.  Its arguments are the
/// kernel's, so they are not read.
#[derive(Debug, Default, clap::Parser)]
#[command(no_binary_name = true)]
struct Environment {
    /// Set by the early-boot stage: this boot has been counted against a
    /// pending trial already
    #[arg(long, env = "DPHONE_EARLY")]
    early: bool,
}

impl Environment {
    fn read() -> Environment {
        <Environment as clap::Parser>::try_parse_from(std::iter::empty::<&str>()).unwrap_or_default()
    }
}

/// Descriptor number the main role finds its heartbeat pipe on.
pub const HEARTBEAT_FD: RawFd = 3;

pub const BEAT_ALIVE: u8 = b'h';
pub const BEAT_HEALTHY: u8 = b'H';
/// Up and reachable, in safe mode.  Never confirms a trial boot.
pub const BEAT_SAFE: u8 = b'S';

pub const START_GRACE_SECS: u64 = 120;
pub const BEAT_TIMEOUT_SECS: u64 = 30;
pub const MAX_BACKOFF_SECS: u64 = 60;
pub const UNHEALTHY_REBOOT_SECS: u64 = 600;
/// Consecutive main-role failures before safe mode is requested.
pub const SAFE_MODE_AFTER_CRASHES: u32 = 3;

/// How often the clock is saved to nvdata while running.
pub const CLOCK_SAVE_SECS: u64 = 30 * 60;
/// A pushed build (`/tmp/opendeskphone`) that fails this many times in a row,
/// each within [`OVERRIDE_QUICK_FAIL_SECS`], is ignored until the next boot.
pub const OVERRIDE_MAX_FAILURES: u32 = 3;
pub const OVERRIDE_QUICK_FAIL_SECS: u64 = 30;

/// Override: when this file exists and is executable, it runs as the main
/// role in place of the installed binary (a build pushed over SSH; `/tmp`
/// is a tmpfs, so it is gone after a reboot).
pub const MAIN_OVERRIDE: &str = "/tmp/opendeskphone";
/// The serial console's UART: UART A of the D40 thru D70, which the EHS
/// headset adapter shares, and on the D80 UART1, which is the console's
/// alone.  The shell on it is started under the same rule on every model
/// (`Supervisor::check_console`).
const SERIAL_CONSOLES: &[&str] = &["/dev/ttyAMA0", "/dev/ttymxc0"];

/// The serial console's device on this phone.
fn serial_console() -> Option<&'static str> {
    SERIAL_CONSOLES.iter().copied().find(|p| Path::new(p).exists())
}
/// Shells for the serial console and SSH, in order of preference.
pub const SHELLS: &[&str] = &["/bin/brush", "/bin/sh"];

/// What other crates plug into the init role.
#[derive(Clone, Copy, Default)]
pub struct Hooks {
    /// Flip `ctrl` back to the previous bank (updater crate).
    pub rollback: Option<RollbackHook>,
}

/// Options for running the supervisor outside PID 1 (host tests).
#[derive(Clone, Debug)]
pub struct TestOptions {
    pub paths: Paths,
    /// Program to run as the main role, with its arguments.
    pub main_command: Vec<String>,
    /// Override the timing constants (seconds): grace, beat timeout,
    /// unhealthy reboot.
    pub start_grace: u64,
    pub beat_timeout: u64,
    pub unhealthy_reboot: u64,
    /// Stands in for the flash: a rollback writes the bank it selects
    /// (`0` or `1`) to this file.  Without it there is no rollback hook.
    pub rollback_marker: Option<PathBuf>,
}

static TEST_ROLLBACK_MARKER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn test_rollback_hook(record: &TrialRecord) -> std::io::Result<()> {
    let path = TEST_ROLLBACK_MARKER.get().ok_or_else(|| std::io::Error::other("no rollback marker"))?;
    std::fs::write(path, record.previous_bank.to_string())
}

/// Entry point as PID 1.  Never returns.
pub fn run(hooks: Hooks) -> ! {
    crate::logging::init_kmsg("dphone-init", log::LevelFilter::Info);
    info!("{} {} init role starting", crate::PRODUCT, crate::VERSION);

    mounts::mount_early();
    let mut wd = Watchdog::open();
    wd.feed();

    let detected = model::detect();
    match detected {
        Some(d) if d.cross_checked => info!("model {}", d.model),
        // The D80's boot loader does not name the model: its devicetree
        // is the one source.
        Some(d) if d.model.family() == model::Family::Touch => info!("model {}", d.model),
        Some(d) => warn!("model {} (command line and devicetree do not both confirm it)", d.model),
        None => error!("model unknown: no digium_board= and no digium compatible"),
    }
    wd.feed();

    let nv_state = nvdata::mount(detected.map(|d| d.model.family()));
    wd.feed();
    // A factory reset asked for before the last shutdown, first thing after
    // the mount as on stock.  Not on the stand-in tmpfs, and not in an image
    // that must leave stock's files as they are.
    if nv_state == NvdataState::Mounted && crate::paths::stock_writes_allowed() {
        nvdata::run_pending_factory_reset(std::path::Path::new(crate::paths::NVDATA));
        wd.feed();
    }

    let paths = Paths::default();
    restore_clock(&paths);

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/sbin/init"));
    let mut sup = Supervisor::new(
        hooks,
        paths,
        wd,
        vec![exe.to_string_lossy().into_owned(), "main".to_string()],
        true,
    );
    sup.model = detected.map(|d| d.model);
    sup.recovery = nv_state == NvdataState::Recovery;
    sup.from_ram = trial::started_from_ram();
    if sup.from_ram {
        info!("started from RAM: the root file system is not a firmware bank; a reboot returns to the installed firmware");
    }
    // Count the boot against a pending trial before anything that touches
    // more hardware: a driver that stalls the phone while its module loads
    // (the watchdog then resets it) must still use up the trial's boots,
    // or the phone would restart into the same image forever.
    sup.begin_trial_accounting();
    sup.wd.feed();
    if let Some(d) = detected {
        modules::load_boot_modules(d.model);
    }
    sup.wd.feed();
    sup.run()
}

/// Run only the supervisor, as an ordinary process (host tests).  Returns
/// the exit code instead of rebooting.
pub fn run_supervisor_test(opts: TestOptions) -> i32 {
    crate::logging::init_ring(log::LevelFilter::Info, true);
    let wd = Watchdog::never();
    let mut hooks = Hooks::default();
    if let Some(marker) = opts.rollback_marker {
        let _ = TEST_ROLLBACK_MARKER.set(marker);
        hooks.rollback = Some(test_rollback_hook);
    }
    let mut sup = Supervisor::new(hooks, opts.paths, wd, opts.main_command, false);
    sup.start_grace = opts.start_grace;
    sup.beat_timeout = opts.beat_timeout;
    sup.unhealthy_reboot = opts.unhealthy_reboot;
    sup.begin_trial_accounting();
    sup.run()
}

fn restore_clock(paths: &Paths) {
    let saved = std::fs::read_to_string(paths.systime()).ok();
    let target = crate::clock::restore_target(saved.as_deref());
    if sys::now_secs() >= target {
        return;
    }
    match sys::set_clock(target) {
        Ok(()) => info!("clock set to {} UTC", crate::clock::format_systime(target)),
        Err(e) => warn!("could not set clock: {e}"),
    }
}

struct MainProc {
    pid: libc::pid_t,
    beat: File,
    started: u64,
    last_beat: Option<u64>,
    healthy: bool,
    /// It reported being up in safe mode.
    safe_up: bool,
    is_override: bool,
    killed: bool,
}

struct Supervisor {
    hooks: Hooks,
    paths: Paths,
    wd: Watchdog,
    main_command: Vec<String>,
    /// True as PID 1 on a phone; false in the host test mode, where nothing
    /// outside our own children may be touched.
    real: bool,
    model: Option<model::Model>,
    recovery: bool,
    /// Started from RAM, not booted from a bank: trial records are not
    /// ours to count or confirm.
    from_ram: bool,
    max_uptime: Option<u64>,
    sigfd: RawFd,
    main: Option<MainProc>,
    next_start: u64,
    crashes: u32,
    override_failures: u32,
    console: Option<libc::pid_t>,
    console_next: u64,
    trial: Option<TrialRecord>,
    last_alive: u64,
    last_clock_save: u64,
    start_grace: u64,
    beat_timeout: u64,
    unhealthy_reboot: u64,
}

impl Supervisor {
    fn new(hooks: Hooks, paths: Paths, wd: Watchdog, main_command: Vec<String>, real: bool) -> Self {
        Supervisor {
            hooks,
            paths,
            wd,
            main_command,
            real,
            model: None,
            recovery: false,
            from_ram: false,
            max_uptime: std::fs::read_to_string("/proc/cmdline")
                .ok()
                .and_then(|c| c.split_ascii_whitespace().find_map(|w| w.strip_prefix("dphone.max_uptime=").and_then(|v| v.parse().ok()))),
            sigfd: -1,
            main: None,
            next_start: 0,
            crashes: 0,
            override_failures: 0,
            console: None,
            console_next: 0,
            trial: None,
            last_alive: sys::monotonic_secs(),
            last_clock_save: sys::monotonic_secs(),
            start_grace: START_GRACE_SECS,
            beat_timeout: BEAT_TIMEOUT_SECS,
            unhealthy_reboot: UNHEALTHY_REBOOT_SECS,
        }
    }

    /// Count this boot against a pending trial and roll back when it has
    /// used up its boots.
    fn begin_trial_accounting(&mut self) {
        if self.recovery {
            // nvdata is a stand-in tmpfs; there is no record to read.
            return;
        }
        if self.from_ram {
            if trial::load(&self.paths.own_dir()).is_some() {
                info!("a trial boot is pending; this system was started from RAM and leaves the record alone");
            }
            return;
        }
        if Environment::read().early {
            // The early-boot stage in the kernel partition counted this
            // boot before the root file system was mounted, and went back
            // itself if the trial was used up.  What is left here is to
            // know that a trial is on: to confirm it, or to go back when
            // the main role keeps failing.
            let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
            let running = opendeskphone_ctrl::boot::running_bank(&read("/proc/cmdline"), &read("/proc/mtd"));
            self.trial = trial::load(&self.paths.own_dir()).filter(|r| trial::is_about(r, running));
            if let Some(record) = &self.trial {
                info!("trial boot {} of {} (previous bank {})", record.boots, record.max_boots, record.previous_bank);
            }
            return;
        }
        self.trial = trial::count_boot(&self.paths.own_dir());
        if let Some(record) = self.trial.clone() {
            info!("trial boot {} of {} (previous bank {})", record.boots, record.max_boots, record.previous_bank);
            if record.exhausted() {
                self.roll_back("trial boots used up");
            }
        }
    }

    /// Ask the updater to restore the previous bank, then reboot.  When the
    /// hook is missing or fails, the trial is abandoned so the phone does not
    /// loop here, and the current bank keeps running.
    fn roll_back(&mut self, why: &str) {
        let Some(record) = self.trial.take() else { return };
        let own_dir = self.paths.own_dir();
        match self.hooks.rollback {
            Some(hook) => match hook(&record) {
                Ok(()) => {
                    error!("{why}: reverted to bank {}", record.previous_bank);
                    let _ = trial::clear(&own_dir);
                    self.shutdown(
                        Shutdown::Reboot,
                        Some(&format!("init: {why}, reverted to bank {}", record.previous_bank)),
                    );
                }
                Err(e) => error!("{why}: rollback to bank {} failed: {e}; staying on this bank", record.previous_bank),
            },
            None => error!("{why}: no rollback hook in this build; staying on this bank"),
        }
        let _ = trial::clear(&own_dir);
    }

    fn setup_signals(&mut self) {
        // SAFETY: sigset operations on a local set; signalfd returns a new
        // descriptor we own.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for sig in
                [libc::SIGCHLD, libc::SIGTERM, libc::SIGUSR1, libc::SIGUSR2, libc::SIGINT, libc::SIGHUP, sys::kexec_signal()]
            {
                libc::sigaddset(&mut set, sig);
            }
            libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            self.sigfd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        }
        if self.sigfd < 0 {
            error!("signalfd: {}", std::io::Error::last_os_error());
        }
    }

    fn run(&mut self) -> ! {
        self.setup_signals();
        loop {
            self.wd.feed();
            let now = sys::monotonic_secs();
            self.reap();
            self.read_beats(now);
            self.check_main(now);
            self.check_console(now);
            self.periodic(now);
            self.wait_for_events();
            self.handle_signals();
        }
    }

    fn wait_for_events(&self) {
        let mut fds = [
            libc::pollfd { fd: self.sigfd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.main.as_ref().map_or(-1, |m| m.beat.as_raw_fd()), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: fds is a valid array of two pollfd.  Negative descriptors
        // are ignored by poll.
        unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
        if self.sigfd < 0 {
            // Without a signalfd, poll returns at once when the beat pipe is
            // at end of file; do not spin.
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    fn handle_signals(&mut self) {
        if self.sigfd < 0 {
            return;
        }
        loop {
            let mut info = [0u8; 128];
            // SAFETY: info is writable for 128 bytes, the size of
            // signalfd_siginfo.
            let n = unsafe { libc::read(self.sigfd, info.as_mut_ptr() as *mut libc::c_void, info.len()) };
            if n != info.len() as isize {
                return;
            }
            let signo = u32::from_ne_bytes([info[0], info[1], info[2], info[3]]) as libc::c_int;
            match signo {
                libc::SIGCHLD => self.reap(),
                libc::SIGTERM | libc::SIGINT => self.shutdown(Shutdown::Reboot, None),
                libc::SIGUSR1 => self.shutdown(Shutdown::Halt, None),
                libc::SIGUSR2 => self.shutdown(Shutdown::PowerOff, None),
                s if s == sys::kexec_signal() => self.start_loaded_kernel(),
                _ => {}
            }
        }
    }

    /// Collect every exited child, ours or adopted.
    fn reap(&mut self) {
        loop {
            let mut status: libc::c_int = 0;
            // In test mode only our own children exist, so -1 is still right.
            // SAFETY: status is a valid out pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            if self.main.as_ref().is_some_and(|m| m.pid == pid) {
                self.main_exited(status);
            } else if self.console == Some(pid) {
                self.console = None;
                self.console_next = sys::monotonic_secs() + 2;
            }
        }
    }

    fn main_exited(&mut self, status: libc::c_int) {
        let Some(mut proc_) = self.main.take() else { return };
        let now = sys::monotonic_secs();
        // Take any last beats (a healthy report just before exit still counts).
        let was = (proc_.healthy, proc_.safe_up);
        Self::drain_beats(&mut proc_, now);
        if proc_.healthy && !was.0 {
            self.note_healthy();
        }
        if proc_.safe_up && !was.1 {
            self.note_safe_up();
        }
        let ran = now.saturating_sub(proc_.started);
        let clean = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
        if libc::WIFSIGNALED(status) {
            error!("main role killed by signal {} after {ran} s", libc::WTERMSIG(status));
        } else if clean {
            info!("main role exited cleanly after {ran} s; restarting");
        } else {
            error!("main role exited with status {} after {ran} s", libc::WEXITSTATUS(status));
        }
        if proc_.is_override && !clean && ran < OVERRIDE_QUICK_FAIL_SECS {
            self.override_failures += 1;
            if self.override_failures == OVERRIDE_MAX_FAILURES {
                warn!("{MAIN_OVERRIDE} failed {OVERRIDE_MAX_FAILURES} times; using the built-in main role");
            }
        } else if proc_.is_override {
            self.override_failures = 0;
        }
        if clean {
            self.next_start = now;
            return;
        }
        // A main role that failed may not come back, and the serial console
        // is then the way in: it gets the UART, whatever the settings say,
        // until a main role that works decides again.
        self.grant_console();
        if proc_.healthy && ran > 300 {
            self.crashes = 0;
        }
        self.crashes += 1;
        let backoff = (1u64 << (self.crashes - 1).min(6)).min(MAX_BACKOFF_SECS);
        self.next_start = now + backoff;
        info!("restarting main role in {backoff} s (failure {} in a row)", self.crashes);
        self.roll_back_if_failing();
    }

    /// During a trial boot, a main role that has failed
    /// [`trial::MAX_TRIAL_CRASHES`] times in a row is reason enough to go
    /// back at once.
    fn roll_back_if_failing(&mut self) {
        if self.trial.is_some() && self.crashes >= trial::MAX_TRIAL_CRASHES {
            self.roll_back("main role keeps failing during trial boot");
        }
    }

    fn drain_beats(proc_: &mut MainProc, now: u64) {
        let mut buf = [0u8; 64];
        loop {
            match proc_.beat.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    proc_.last_beat = Some(now);
                    if buf[..n].contains(&BEAT_HEALTHY) {
                        proc_.healthy = true;
                    }
                    if buf[..n].contains(&BEAT_SAFE) {
                        proc_.safe_up = true;
                    }
                }
                Err(_) => return,
            }
        }
    }

    fn read_beats(&mut self, now: u64) {
        let Some(proc_) = self.main.as_mut() else { return };
        let was = (proc_.healthy, proc_.safe_up);
        Self::drain_beats(proc_, now);
        if proc_.last_beat == Some(now) {
            self.last_alive = now;
        }
        let is = (proc_.healthy, proc_.safe_up);
        if is.0 && !was.0 {
            info!("main role reports healthy");
            self.note_healthy();
        }
        if is.1 && !was.1 {
            info!("main role is up in safe mode");
            self.note_safe_up();
        }
    }

    /// A safe-mode main role is up.  That ends the run of failures, so the
    /// full main role gets another try after the next exit, but it says
    /// nothing about the image on trial: the record stays, and during a
    /// trial the failures keep counting toward the rollback.
    fn note_safe_up(&mut self) {
        if self.trial.is_none() {
            self.crashes = 0;
        }
    }

    fn note_healthy(&mut self) {
        self.crashes = 0;
        if self.trial.take().is_some() {
            match trial::clear(&self.paths.own_dir()) {
                Ok(()) => info!("trial boot confirmed"),
                Err(e) => warn!("could not clear trial record: {e}"),
            }
        }
    }

    fn check_main(&mut self, now: u64) {
        match self.main.as_mut() {
            Some(proc_) if proc_.killed => {}
            Some(proc_) => {
                let (since, limit) = match proc_.last_beat {
                    Some(t) => (t, self.beat_timeout),
                    None => (proc_.started, self.start_grace),
                };
                if now.saturating_sub(since) > limit {
                    error!("main role silent for more than {limit} s; killing it");
                    let _ = sys::kill(proc_.pid, libc::SIGKILL);
                    proc_.killed = true;
                    // SIGCHLD follows; main_exited() counts it as a failure.
                }
            }
            None if now >= self.next_start => self.spawn_main(now),
            None => {}
        }
        let hold = self.paths.run.join("no-reboot").exists();
        if !hold && now.saturating_sub(self.last_alive) > self.unhealthy_reboot {
            error!("no heartbeat from the main role for {} s; rebooting", self.unhealthy_reboot);
            self.shutdown(Shutdown::Reboot, Some("init: main role not running"));
        }
    }

    fn main_program(&self) -> (Vec<String>, bool) {
        if self.real && self.override_failures < OVERRIDE_MAX_FAILURES && is_executable(Path::new(MAIN_OVERRIDE)) {
            return (vec![MAIN_OVERRIDE.to_string(), "main".to_string()], true);
        }
        (self.main_command.clone(), false)
    }

    fn spawn_main(&mut self, now: u64) {
        let (argv, is_override) = self.main_program();
        let Some(program) = argv.first() else { return };
        let mut fds = [0 as RawFd; 2];
        // SAFETY: fds is a valid two-element array.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            error!("pipe: {}", std::io::Error::last_os_error());
            self.next_start = now + 5;
            return;
        }
        let (rd, wr) = (fds[0], fds[1]);
        // SAFETY: rd is a fresh descriptor we own.
        let beat = unsafe { File::from_raw_fd(rd) };
        let _ = sys::set_nonblocking(rd, true);

        let mut cmd = Command::new(program);
        cmd.args(&argv[1..]);
        if self.real {
            cmd.arg0("dphone");
        }
        cmd.args(["--heartbeat-fd", &HEARTBEAT_FD.to_string()]);
        if self.recovery {
            cmd.args(["--recovery", "nvdata"]);
        }
        if let Some(m) = self.model {
            cmd.args(["--model", m.name()]);
        }
        // After repeated failures, ask for the reduced main role (SSH,
        // network, diagnostics only), so a broken subsystem cannot keep the
        // phone unreachable.  Not during a trial boot: the image on trial
        // has to show that its full main role works, and one that keeps
        // failing is rolled back instead (`roll_back_if_failing`).
        if self.crashes >= SAFE_MODE_AFTER_CRASHES && self.trial.is_none() {
            warn!("main role failed {} times in a row; starting it in safe mode", self.crashes);
            cmd.arg("--safe");
        }
        cmd.stdin(std::process::Stdio::null());
        // Our own standard output and error are the console UART.  The main
        // role inherits them only while the serial console has the UART;
        // otherwise its log copies would reach an EHS adapter.  It moves
        // them itself when the setting says so (`sys::stdio_to_console`).
        if self.real && !self.paths.serial_console().exists() {
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());
        }
        // SAFETY: the closure runs between fork and exec and makes only
        // async-signal-safe calls.
        unsafe {
            cmd.pre_exec(move || {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                if wr == HEARTBEAT_FD {
                    let flags = libc::fcntl(wr, libc::F_GETFD);
                    libc::fcntl(wr, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                } else if libc::dup2(wr, HEARTBEAT_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        // SAFETY: wr is ours and no longer needed in this process.
        unsafe { libc::close(wr) };
        match spawned {
            Ok(child) => {
                let pid = child.id() as libc::pid_t;
                // The child is reaped by waitpid(-1) in reap(); dropping the
                // handle does not wait.
                drop(child);
                info!("started main role: {} (pid {pid})", argv.join(" "));
                self.main = Some(MainProc { pid, beat, started: now, last_beat: None, healthy: false, safe_up: false, is_override, killed: false });
            }
            Err(e) => {
                error!("cannot start main role {program}: {e}");
                if is_override {
                    self.override_failures += 1;
                }
                self.crashes += 1;
                self.next_start = now + (1u64 << (self.crashes - 1).min(6)).min(MAX_BACKOFF_SECS);
                self.roll_back_if_failing();
            }
        }
    }

    /// Give UART A to the serial console: kernel messages go to it, and
    /// [`Self::check_console`] starts the shell.
    fn grant_console(&mut self) {
        if self.paths.serial_console().exists() {
            return;
        }
        info!("the serial console gets the UART");
        if let Err(e) = std::fs::write(self.paths.serial_console(), b"init\n") {
            warn!("{}: {e}", self.paths.serial_console().display());
        }
        if self.real {
            sys::serial_console_output(true);
        }
    }

    /// Serial console shell on the UART, while the console has it.  By
    /// default it does not: the UART is the EHS headset adapter's, and
    /// nothing may be written to it.  The main role says otherwise through
    /// [`Paths::serial_console`] (the `serial_console` setting).
    fn check_console(&mut self, now: u64) {
        if !self.real {
            return;
        }
        let granted = self.paths.serial_console().exists();
        match self.console {
            Some(pid) if !granted => {
                info!("serial console released to EHS");
                let _ = sys::kill(pid, libc::SIGHUP);
                let _ = sys::kill(pid, libc::SIGKILL);
            }
            None if granted && now >= self.console_next => {
                self.console_next = now + 10;
                let Some(shell) = SHELLS.iter().find(|s| Path::new(s).exists()) else { return };
                let Some(tty) = serial_console() else { return };
                match spawn_console(shell, tty, &self.paths) {
                    Ok(pid) => self.console = Some(pid),
                    Err(e) => warn!("serial console: {e}"),
                }
            }
            _ => {}
        }
    }

    fn periodic(&mut self, now: u64) {
        // Test boots: `dphone.max_uptime=SECS` on the kernel command line ends
        // the boot after that long unless `/run/opendeskphone/stay` exists, so
        // a RAM-booted trial returns to the installed firmware by itself even
        // when nobody can reach it.  It ends by no longer feeding the
        // watchdog, which then resets the phone.
        if self.real
            && let Some(limit) = self.max_uptime
            && now >= limit
            && !Path::new(crate::paths::RUN_DIR).join("stay").exists()
        {
            warn!("dphone.max_uptime={limit} reached: syncing and letting the watchdog reset the phone");
            self.max_uptime = None;
            sys::sync();
            self.wd.starve();
        }
        // A trial boot that has not come up healthy in time is restarted, so
        // that an image whose main role runs but cannot be reached (e.g., no
        // network) still uses up its boots and is rolled back.
        if self.real && self.trial.is_some() && now >= trial::TRIAL_HEALTHY_SECS {
            error!("trial boot not healthy after {} s; rebooting", trial::TRIAL_HEALTHY_SECS);
            self.trial = None;
            self.shutdown(Shutdown::Reboot, Some("init: trial boot did not become healthy"));
        }
        if now.saturating_sub(self.last_clock_save) >= CLOCK_SAVE_SECS {
            self.last_clock_save = now;
            self.save_clock();
        }
    }

    fn save_clock(&self) {
        if self.recovery || (self.real && !crate::paths::stock_writes_allowed()) {
            return;
        }
        let wall = sys::now_secs();
        // A clock nothing has corrected is not worth a flash write, unless
        // there is no file at all.
        if wall <= crate::clock::FLOOR && self.paths.systime().exists() {
            return;
        }
        if let Err(e) = crate::clock::save(&self.paths.systime(), wall) {
            warn!("saving clock: {e}");
        }
    }

    /// `dphone ramboot` has loaded a kernel and asks for the jump.  Refused
    /// (the phone keeps running) when no image is loaded, or when the
    /// watchdog is not armed: the watchdog is what brings the installed
    /// firmware back if the new kernel hangs, since boot2 has none.
    fn start_loaded_kernel(&mut self) {
        if !self.real {
            return;
        }
        if std::fs::read_to_string(sys::KEXEC_LOADED).map_or(true, |s| s.trim() != "1") {
            error!("asked to start a loaded kernel, but none is loaded; ignored");
            return;
        }
        if !self.wd.is_armed() {
            error!("asked to start a loaded kernel, but the watchdog is not armed; ignored");
            sys::kexec_unload();
            return;
        }
        self.shutdown(Shutdown::Kexec, None)
    }

    /// Stop everything and reboot, halt, power off, or start the loaded
    /// kernel.  Never returns.
    fn shutdown(&mut self, kind: Shutdown, reason: Option<&str>) -> ! {
        info!("shutting down ({kind:?})");
        let may_write = !self.recovery && !(self.real && !crate::paths::stock_writes_allowed());
        if let (Some(reason), true) = (reason, may_write) {
            let _ = crate::paths::write_atomic(&self.paths.reboot_reason(), format!("{reason}\n").as_bytes(), 0o644);
        }
        if let Some(proc_) = self.main.as_ref() {
            let _ = sys::kill(proc_.pid, libc::SIGTERM);
        }
        self.wait_children(50, |s| s.main.is_none());
        if let Some(proc_) = self.main.as_ref() {
            let _ = sys::kill(proc_.pid, libc::SIGKILL);
            self.wait_children(10, |s| s.main.is_none());
        }
        if !self.real {
            std::process::exit(0);
        }
        // Everything else: ask, then insist.
        let _ = sys::kill(-1, libc::SIGTERM);
        self.wait_children(10, |_| false);
        let _ = sys::kill(-1, libc::SIGKILL);
        self.wait_children(3, |_| false);
        self.save_clock();
        mounts::unmount_for_shutdown();
        self.wd.feed();
        match kind {
            Shutdown::Reboot => {}
            // Nothing will feed the watchdog after a halt; try to disarm it.
            Shutdown::Halt | Shutdown::PowerOff => self.wd.disarm(),
            // The watchdog stays armed across the jump, with time for the
            // image to be copied into place and the new kernel to reach
            // its watchdog driver.  nvdata is unmounted and every process
            // is gone, so the flash is idle when the kernel is left.
            Shutdown::Kexec => {
                if !self.wd.set_timeout(watchdog::KEXEC_TIMEOUT_SECS) {
                    warn!("could not lengthen the watchdog timeout for the jump");
                }
                self.wd.feed();
                info!("starting the loaded kernel");
            }
        }
        let err = sys::reboot(kind);
        error!("reboot system call failed: {err}");
        // Stop feeding: the watchdog resets the phone.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// Reap for up to `tenths` tenths of a second, feeding the watchdog,
    /// until `done` says so.
    fn wait_children(&mut self, tenths: u32, done: impl Fn(&Self) -> bool) {
        // While shutting down, exits must not schedule restarts or rollbacks;
        // main_exited() only records state, and nothing restarts it because
        // check_main() is not called from here.
        for _ in 0..tenths {
            self.reap_quiet();
            if done(self) {
                return;
            }
            self.wd.feed();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.reap_quiet();
    }

    fn reap_quiet(&mut self) {
        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: status is a valid out pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            if self.main.as_ref().is_some_and(|m| m.pid == pid) {
                self.main = None;
            } else if self.console == Some(pid) {
                self.console = None;
            }
        }
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Environment for interactive shells on the phone.
pub fn shell_environment(paths: &Paths, shell: &str) -> Vec<(String, String)> {
    let home = paths.home();
    let home = if home.is_dir() { home.to_string_lossy().into_owned() } else { "/".to_string() };
    vec![
        ("PATH".into(), "/sbin:/usr/sbin:/bin:/usr/bin".into()),
        ("HOME".into(), home),
        ("USER".into(), "root".into()),
        ("LOGNAME".into(), "root".into()),
        ("SHELL".into(), shell.into()),
    ]
}

fn spawn_console(shell: &str, tty: &str, paths: &Paths) -> std::io::Result<libc::pid_t> {
    let tty = CString::new(tty).map_err(|_| std::io::Error::other("bad tty path"))?;
    let mut cmd = Command::new(shell);
    cmd.env_clear();
    cmd.envs(shell_environment(paths, shell));
    cmd.env("TERM", "vt100");
    cmd.current_dir("/");
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            libc::setsid();
            let fd = libc::open(tty.as_ptr(), libc::O_RDWR);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::ioctl(fd, libc::TIOCSCTTY as _, 1);
            libc::dup2(fd, 0);
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            if fd > 2 {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(child.id() as libc::pid_t)
}
