//! The D80's recovery: `/init` of the ramdisk in the recovery image.
//!
//! It does what the stock recovery does when the stock firmware has left
//! it a package, and looks like it, with our logo for the robot:
//!
//! 1. It mounts the stock `/cache` partition (`dphone`'s nvdata) and reads
//!    `recovery/command` there.
//! 2. With `--update_package=PATH` in it, it checks the package's signature
//!    against its keys and runs the package's `update-binary`
//!    (`opendeskphone_recovery::install`), with a bar on the screen.
//! 3. It writes `recovery/last_install` and `recovery/last_log`, removes
//!    the command (and the package, once installed), sets the boot
//!    loader's `bootcmd` back to the system, and restarts.
//!
//! Like the stock recovery on this phone, it does not wait for anyone: the
//! D80 has no keys.  With no command, or after a package that could not be
//! installed, it says so on the screen for a few seconds and restarts into
//! the system all the same.
//!
//! For debugging, a root shell runs on the serial console (`ttymxc0`) for
//! as long as the recovery does.  While the file `/tmp/stay` exists the
//! recovery does not restart, so `: > /tmp/stay` in that shell keeps it,
//! and `reboot` leaves.  `bootcmd` is set back before the wait: a power
//! cycle starts the system.

use opendeskphone_hal::phone::DrmDisplay;
use opendeskphone_hal::{Display, FrameInfo, Model, Screen as Panel};
use opendeskphone_recovery::install::{self, Log};
use opendeskphone_recovery::{command, env, props, sig};
use std::ffi::CString;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A picture: red, green, blue, and alpha for every pixel, the colors
/// multiplied by the alpha.
pub struct Picture {
    width: u32,
    height: u32,
    pixels: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/art.rs"));

/// The serial console, for the shell.
const TTY: &str = "/dev/ttymxc0";
const SHELL: &str = "/bin/sh";
/// The recovery's log while it runs, where the stock recovery has it.
const LOG: &str = "/tmp/recovery.log";
/// While this file exists, the recovery does not restart.
const STAY: &str = "/tmp/stay";
/// The stock firmware's name for the D80, which a package's script asks
/// for before it writes anything.
const PROPERTIES: &[(&str, &str)] = &[("ro.product.device", opendeskphone_recovery::ANDROID_DEVICE), ("ro.build.product", opendeskphone_recovery::ANDROID_DEVICE)];

/// The colors of the D80's skin: the background, the bar's track, and the
/// bar (the blue of the logo).
const BACKGROUND: [u8; 3] = [0x21, 0x21, 0x21];
const TRACK: [u8; 3] = [0x4d, 0x4d, 0x4d];
const BAR: [u8; 3] = [0x2f, 0x80, 0xed];

/// What the screen says under the logo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Installing,
    NoCommand,
    Error,
    Restarting,
}

/// The screen: the logo, a bar, and one line of text, in one block in the
/// middle, as `dphone` has its start-up screen.
struct Screen {
    display: DrmDisplay,
    info: FrameInfo,
    /// The bar's place, and how many of its pixels are filled.
    bar: (usize, usize, usize, usize),
    filled: Option<usize>,
}

impl Screen {
    const GAP_BAR: usize = 60;
    const BAR_HEIGHT: usize = 16;
    const GAP_TEXT: usize = 24;
    const TEXT_HEIGHT: usize = 56;

    fn open() -> Result<Screen, String> {
        let display = DrmDisplay::open(Model::D80).map_err(|e| e.to_string())?;
        let info = display.info(Panel::Main).ok_or("no screen")?;
        let (w, h) = (info.width as usize, info.height as usize);
        let block = LOGO.height as usize + Self::GAP_BAR + Self::BAR_HEIGHT + Self::GAP_TEXT + Self::TEXT_HEIGHT;
        let top = h.saturating_sub(block) / 2;
        let mut screen =
            Screen { display, info, bar: (w / 4, top + LOGO.height as usize + Self::GAP_BAR, w / 2, Self::BAR_HEIGHT), filled: None };
        screen.fill(0, 0, w, h, BACKGROUND);
        screen.picture(&LOGO, w.saturating_sub(LOGO.width as usize) / 2, top);
        // The first picture is in the frame before the panel is lit.
        screen.display.set_power(Panel::Main, true).map_err(|e| e.to_string())?;
        screen.flush();
        Ok(screen)
    }

    fn flush(&mut self) {
        let _ = self.display.flush(Panel::Main, &[]);
    }

    /// One row of the frame, from `x`, `w` pixels.
    fn row(&mut self, x: usize, y: usize, w: usize) -> &mut [u8] {
        let (stride, width, height) = (self.info.stride, self.info.width as usize, self.info.height as usize);
        let Ok(frame) = self.display.frame(Panel::Main) else { return &mut [] };
        if y >= height || x >= width {
            return &mut [];
        }
        let w = w.min(width - x);
        frame.get_mut(y * stride + 4 * x..y * stride + 4 * (x + w)).unwrap_or(&mut [])
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, [r, g, b]: [u8; 3]) {
        for y in y..y + h {
            for pixel in self.row(x, y, w).as_chunks_mut::<4>().0 {
                *pixel = [b, g, r, 0xff];
            }
        }
    }

    /// A bar with round ends.
    fn pill(&mut self, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
        let radius = h as f32 / 2.0;
        for row in 0..h {
            let from_middle = (row as f32 + 0.5 - radius).abs();
            let inset = (radius - (radius * radius - from_middle * from_middle).max(0.0).sqrt()).round() as usize;
            if w > 2 * inset {
                self.fill(x + inset, y + row, w - 2 * inset, 1, color);
            }
        }
    }

    fn picture(&mut self, picture: &Picture, x: usize, y: usize) {
        let w = picture.width as usize;
        for (line, source) in picture.pixels.chunks_exact(4 * w).enumerate() {
            for (pixel, s) in self.row(x, y + line, w).as_chunks_mut::<4>().0.iter_mut().zip(source.as_chunks::<4>().0) {
                let keep = 255 - u32::from(s[3]);
                for (to, from) in [(0, 2), (1, 1), (2, 0)] {
                    pixel[to] = (u32::from(s[from]) + u32::from(pixel[to]) * keep / 255).min(255) as u8;
                }
            }
        }
    }

    fn stage(&mut self, stage: Stage) {
        let text = match stage {
            Stage::Installing => &INSTALLING,
            Stage::NoCommand => &NO_COMMAND,
            Stage::Error => &ERROR,
            Stage::Restarting => &RESTARTING,
        };
        let (width, top) = (self.info.width as usize, self.bar.1 + Self::BAR_HEIGHT + Self::GAP_TEXT);
        self.fill(0, top, width, Self::TEXT_HEIGHT, BACKGROUND);
        self.picture(text, width.saturating_sub(text.width as usize) / 2, top + Self::TEXT_HEIGHT.saturating_sub(text.height as usize) / 2);
        // The bar is for an installation.
        if stage != Stage::Installing {
            let (x, y, w, h) = self.bar;
            self.fill(x, y, w, h, BACKGROUND);
            self.filled = None;
        }
        self.flush();
    }

    fn bar(&mut self, position: f32) {
        let (x, y, w, h) = self.bar;
        // The fill keeps its round ends: it is never shorter than it is tall.
        let filled = h + ((w - h) as f32 * position.clamp(0.0, 1.0)) as usize;
        if self.filled == Some(filled) {
            return;
        }
        self.filled = Some(filled);
        self.pill(x, y, w, h, TRACK);
        if position > 0.0 {
            self.pill(x, y, filled, h, BAR);
        }
        self.flush();
    }
}

/// What an installation shows: its texts go to the log, as on the stock
/// recovery, whose screen has no text unless a key asks for it.
struct Shown<'a> {
    screen: Option<&'a mut Screen>,
    log: Log,
}

impl install::Ui for Shown<'_> {
    fn print(&mut self, text: &str) {
        (self.log)(text);
    }
    fn progress(&mut self, position: f32) {
        if let Some(screen) = &mut self.screen {
            screen.bar(position);
        }
    }
}

fn cstring(s: &str) -> CString {
    CString::new(s).expect("no NUL in a path")
}

fn mount(source: &str, target: &str, kind: &str, flags: libc::c_ulong) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(target);
    let (source, target, kind) = (cstring(source), cstring(target), cstring(kind));
    // SAFETY: three NUL-terminated strings, and no data.
    match unsafe { libc::mount(source.as_ptr(), target.as_ptr(), kind.as_ptr(), flags, std::ptr::null()) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// Writes what is in memory to the eMMC, and restarts the phone.
fn restart() -> ! {
    // SAFETY: neither call takes memory of ours.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
    // Not reached with the right to restart; a kernel panic otherwise, as
    // this is process 1.
    std::process::exit(1)
}

/// The file systems of a system with nothing mounted, the console as
/// standard input and output, and the names an Android program expects.
fn system() {
    let _ = mount("proc", "/proc", "proc", 0);
    let _ = mount("sysfs", "/sys", "sysfs", 0);
    let _ = mount("devtmpfs", "/dev", "devtmpfs", 0);
    let _ = mount("tmpfs", "/tmp", "tmpfs", 0);
    let console = cstring("/dev/console");
    // SAFETY: opening a device, and making it descriptors 0 to 2.
    unsafe {
        let fd = libc::open(console.as_ptr(), libc::O_RDWR);
        if fd >= 0 {
            for to in 0..3 {
                libc::dup2(fd, to);
            }
            if fd > 2 {
                libc::close(fd);
            }
        }
    }
}

/// Waits for the eMMC's partitions, and gives them their Android names:
/// `/dev/block/mmcblk0p5` for `/dev/mmcblk0p5`.
fn block_devices(log: &Log) {
    let cache = Path::new(opendeskphone_recovery::CACHE_PARTITION);
    for _ in 0..300 {
        if cache.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !cache.exists() {
        log(&format!("{} has not appeared", cache.display()));
    }
    let _ = std::fs::create_dir_all("/dev/block");
    for entry in std::fs::read_dir("/dev").into_iter().flatten().flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("mmcblk") {
            let _ = std::os::unix::fs::symlink(entry.path(), Path::new("/dev/block").join(name));
        }
    }
}

/// The root shell on the serial console, started again whenever it ends.
fn shell(log: Log) {
    if !Path::new(SHELL).exists() {
        return;
    }
    std::thread::spawn(move || {
        loop {
            let tty = cstring(TTY);
            let mut shell = std::process::Command::new(SHELL);
            shell.env_clear().envs([("PATH", "/bin"), ("HOME", "/tmp"), ("TERM", "vt100"), ("SHELL", SHELL)]).current_dir("/");
            // SAFETY: only async-signal-safe calls between fork and exec:
            // a session of its own, with the serial port as its terminal.
            unsafe {
                shell.pre_exec(move || {
                    libc::setsid();
                    let fd = libc::open(tty.as_ptr(), libc::O_RDWR);
                    if fd < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::ioctl(fd, libc::TIOCSCTTY as _, 1);
                    for to in 0..3 {
                        libc::dup2(fd, to);
                    }
                    if fd > 2 {
                        libc::close(fd);
                    }
                    Ok(())
                });
            }
            match shell.spawn().and_then(|mut child| child.wait()) {
                Ok(_) => std::thread::sleep(Duration::from_secs(1)),
                Err(e) => {
                    log(&format!("{SHELL} on {TTY}: {e}"));
                    return;
                }
            }
        }
    });
}

/// How a run of the recovery ended.
enum Outcome {
    NoCommand,
    Installed,
    Failed,
}

/// Reads the command, and installs the package it names.
fn run(screen: &mut Option<Screen>, log: &Log) -> Outcome {
    let dir = Path::new(command::CACHE).join(command::DIR);
    if let Err(e) = mount(opendeskphone_recovery::CACHE_PARTITION, command::CACHE, "ext4", libc::MS_NOSUID | libc::MS_NODEV) {
        log(&format!("{} on {}: {e}", opendeskphone_recovery::CACHE_PARTITION, command::CACHE));
        return Outcome::NoCommand;
    }
    let Some(text) = install::read_optional(&dir.join(command::COMMAND)) else {
        log("no command");
        return Outcome::NoCommand;
    };
    let args = command::parse(&text);
    for ignored in &args.ignored {
        log(&format!("command: \"{ignored}\" is not acted on"));
    }
    let Some(package) = args.update_package.map(PathBuf::from) else {
        log("the command names no package");
        return Outcome::NoCommand;
    };
    log(&format!("command: install {}", package.display()));
    if let Some(screen) = screen {
        screen.stage(Stage::Installing);
        screen.bar(0.0);
    }
    let result = install::install(&package, &sig::keys(), Path::new("/tmp"), &mut Shown { screen: screen.as_mut(), log: log.clone() }, log);
    let _ = std::fs::write(dir.join(command::LAST_INSTALL), format!("{}\n{}\n", package.display(), u8::from(result.is_ok())));
    match result {
        Ok(()) => {
            log("the package is installed");
            // It is of no more use, and it is large.
            let _ = std::fs::remove_file(&package);
            Outcome::Installed
        }
        Err(e) => {
            log(&format!("Installation aborted: {e}"));
            Outcome::Failed
        }
    }
}

/// Everything after the command: the log and the command's end in
/// `/cache`, and the boot loader told to start the system again.
fn finish(log: &Log) {
    match env::set_bootcmd(Path::new(opendeskphone_recovery::DISK), env::BOOT_SYSTEM) {
        Ok(true) => log(&format!("bootcmd is \"{}\" again", env::BOOT_SYSTEM)),
        Ok(false) => log(&format!("bootcmd was \"{}\" already", env::BOOT_SYSTEM)),
        // U-Boot then uses the environment it was built with, which starts
        // the system.
        Err(e) => log(&format!("bootcmd is left as it is: {e}")),
    }
    let dir = Path::new(command::CACHE).join(command::DIR);
    if dir.is_dir() {
        let _ = std::fs::remove_file(dir.join(command::COMMAND));
        let _ = std::fs::copy(LOG, dir.join(command::LAST_LOG));
    }
    // SAFETY: no memory of ours.
    unsafe { libc::sync() };
    let target = cstring(command::CACHE);
    // SAFETY: a NUL-terminated path.
    unsafe { libc::umount(target.as_ptr()) };
}

fn main() {
    let name = std::env::args().next().unwrap_or_default();
    if name.rsplit('/').next() == Some("reboot") {
        restart();
    }
    if std::process::id() != 1 {
        eprintln!("recovery: this is the init of the D80's recovery image, and does nothing when it is not process 1");
        std::process::exit(2);
    }
    system();
    let file = Mutex::new(std::fs::File::create(LOG).ok());
    let log: Log = Arc::new(move |line: &str| {
        println!("recovery: {line}");
        if let Ok(mut file) = file.lock()
            && let Some(file) = file.as_mut()
        {
            let _ = writeln!(file, "{line}");
        }
    });
    log(&format!("Open Desk Phone recovery {}", env!("CARGO_PKG_VERSION")));
    block_devices(&log);
    match props::area(PROPERTIES).map(|area| std::fs::write(props::FILE, area)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log(&format!("{}: {e}", props::FILE)),
        Err(e) => log(&e),
    }
    shell(log.clone());

    // Whatever goes wrong, the phone must get back to its system: a
    // recovery that stops here is started again at every power-on.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut screen = match Screen::open() {
            Ok(screen) => Some(screen),
            Err(e) => {
                log(&format!("no screen: {e}"));
                None
            }
        };
        let outcome = run(&mut screen, &log);
        (outcome, screen)
    }));
    finish(&log);
    let (shown, seconds) = match &outcome {
        Ok((Outcome::Installed, _)) => (Stage::Restarting, 2),
        Ok((Outcome::NoCommand, _)) => (Stage::NoCommand, 5),
        Ok((Outcome::Failed, _)) | Err(_) => (Stage::Error, 15),
    };
    let mut screen = outcome.ok().and_then(|(_, screen)| screen);
    if let Some(screen) = &mut screen {
        screen.stage(shown);
    }
    std::thread::sleep(Duration::from_secs(seconds));
    if Path::new(STAY).exists() {
        log(&format!("{STAY} exists: not restarting until it is removed ('reboot' restarts now)"));
        while Path::new(STAY).exists() {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    log("Rebooting...");
    if let Some(screen) = &mut screen
        && shown != Stage::Restarting
    {
        screen.stage(Stage::Restarting);
    }
    restart();
}
