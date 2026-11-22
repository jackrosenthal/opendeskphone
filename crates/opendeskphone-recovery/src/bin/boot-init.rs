//! The D80's system boot: `/init` of the ramdisk in the boot image.
//!
//! The kernel runs it in place of mounting the root file system, and it
//! does what the kernel would have done, after a one-second window in which
//! a root shell can be asked for on either serial port:
//!
//! 1. It mounts `/dev`, opens the debug console (`ttymxc0`, CN1) and the
//!    EHS port (`ttymxc1`, J508) at 115200 baud, 8N1, and listens on both
//!    for one second.  It says so on the debug console only: whatever is
//!    plugged into the EHS port is not written to unless it asks.
//! 2. When [`RUN`] bytes in a row on one port are [`TRIGGER`] (Ctrl-R), it
//!    waits for the port to go quiet, and runs a root shell on that port,
//!    in the ramdisk: the shell and a few utilities, `mount`, `umount`, and
//!    `reboot`.  The system's partitions are not mounted.  When the shell
//!    ends, the system starts.
//! 3. Otherwise, it mounts the root file system named on the command line
//!    (`root=`, `rootfstype=`, `ro`), moves `/dev` into it, removes the
//!    ramdisk's files, makes the new root `/`, and runs its init (`init=`,
//!    or `/sbin/init`) as process 1, as the kernel does.  When the root
//!    does not mount, the shell runs on the debug console, and the boot is
//!    tried again when it ends.
//!
//! This is a way back to a phone whose system does not start, or does not
//! let anyone in: the boot image is written with the system, and the
//! window is before anything of the system runs.

use std::ffi::CString;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// The debug console, and the EHS port.
const CONSOLE: &str = "/dev/ttymxc0";
const PORTS: &[&str] = &[CONSOLE, "/dev/ttymxc1"];
/// The byte that asks for the shell (Ctrl-R), and how many of it in a row
/// do: neither noise at another speed nor an EHS adapter's packets, which
/// are framed in STX and ETX around hexadecimal text, send three of it.
const TRIGGER: u8 = 0x12;
const RUN: usize = 3;
/// How long the ports are listened to.
const WINDOW: Duration = Duration::from_secs(1);
/// After the trigger, the port is told to stop, and must be quiet this long
/// before the shell starts, so that a held key does not type into the
/// shell; but not longer than `DRAIN_MAX` is waited.
const QUIET: Duration = Duration::from_millis(300);
const DRAIN_MAX: Duration = Duration::from_secs(30);
/// How long the root device is waited for (`rootwait`).
const ROOT_WAIT: Duration = Duration::from_secs(30);
const NEW_ROOT: &str = "/root";
const SHELL: &str = "/bin/sh";
/// What the kernel tries when there is no `init=`.
const INITS: &[&str] = &["/sbin/init", "/etc/init", "/bin/init", "/bin/sh"];

fn cstring(s: &str) -> CString {
    CString::new(s).expect("no NUL in a path")
}

fn os(result: libc::c_int) -> std::io::Result<()> {
    if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

fn mount(source: &str, target: &str, kind: &str, flags: libc::c_ulong, data: &str) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(target);
    let (source, target, kind, data) = (cstring(source), cstring(target), cstring(kind), cstring(data));
    let data = if data.is_empty() { std::ptr::null() } else { data.as_ptr().cast() };
    // SAFETY: NUL-terminated strings, and data that is text or nothing.
    os(unsafe { libc::mount(source.as_ptr(), target.as_ptr(), kind.as_ptr(), flags, data) })
}

fn umount(target: &str, flags: libc::c_int) -> std::io::Result<()> {
    let target = cstring(target);
    // SAFETY: a NUL-terminated path.
    os(unsafe { libc::umount2(target.as_ptr(), flags) })
}

/// One line in the kernel's log, which the console shows; on the console
/// itself when there is no log.
fn say(line: &str) {
    let line = format!("boot: {line}\n");
    match std::fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        Ok(mut kmsg) => drop(kmsg.write_all(line.as_bytes())),
        Err(_) => drop(std::io::stdout().write_all(line.as_bytes())),
    }
}

fn restart() -> ! {
    // SAFETY: neither call takes memory of ours.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
    std::process::exit(1)
}

/// `/dev`, and the console as standard input and output.  The kernel opens
/// `/dev/console` for init before there is a `/dev`, so there is none yet.
fn devices() {
    let _ = mount("devtmpfs", "/dev", "devtmpfs", libc::MS_NOSUID, "mode=0755");
    let console = cstring("/dev/console");
    // SAFETY: opening a device, and making it descriptors 0 to 2.
    unsafe {
        let fd = libc::open(console.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
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

// ------------------------------------------------------------ the window

/// A serial port as it was, so that it can be left so.
struct Port {
    path: &'static str,
    fd: OwnedFd,
    saved: libc::termios,
    /// How many of the trigger in a row the port has had.
    run: usize,
}

impl Port {
    fn open(path: &'static str) -> Option<Port> {
        let name = cstring(path);
        // SAFETY: a NUL-terminated path; the descriptor is owned below.
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            say(&format!("{path}: {}", std::io::Error::last_os_error()));
            return None;
        }
        // SAFETY: just opened, and owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: termios is plain data, which tcgetattr fills.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: an open descriptor and a termios to fill.
        if unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut saved) } != 0 {
            say(&format!("{path}: {}", std::io::Error::last_os_error()));
            return None;
        }
        let mut raw = saved;
        // SAFETY: changing a copy, and setting it on the open descriptor.
        unsafe {
            libc::cfmakeraw(&mut raw);
            raw.c_cflag |= libc::CLOCAL | libc::CREAD;
            libc::cfsetispeed(&mut raw, libc::B115200);
            libc::cfsetospeed(&mut raw, libc::B115200);
            libc::tcsetattr(fd.as_raw_fd(), libc::TCSANOW, &raw);
        }
        Some(Port { path, fd, saved, run: 0 })
    }

    /// What has arrived; `true` when it ends the trigger's run.
    fn read(&mut self) -> bool {
        let mut buf = [0u8; 64];
        // SAFETY: a buffer of that length.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        for &byte in buf.iter().take(n.max(0) as usize) {
            self.run = if byte == TRIGGER { self.run + 1 } else { 0 };
            if self.run >= RUN {
                return true;
            }
        }
        false
    }

    fn write(&self, text: &str) {
        // SAFETY: the bytes of the text.
        unsafe { libc::write(self.fd.as_raw_fd(), text.as_ptr().cast(), text.len()) };
    }

    /// Asks for the key to be let go, reads until nothing has arrived for
    /// `QUIET`, and drops what is left.
    fn drain(&mut self) {
        self.write("\r\nboot: release the key; the shell starts when this port is quiet\r\n");
        let start = Instant::now();
        let mut buf = [0u8; 64];
        while start.elapsed() < DRAIN_MAX {
            let mut poll = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd.
            if unsafe { libc::poll(&mut poll, 1, QUIET.as_millis() as libc::c_int) } <= 0 {
                break;
            }
            // SAFETY: a buffer of that length.
            if unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } <= 0 {
                break;
            }
        }
        // SAFETY: an open descriptor.
        unsafe { libc::tcflush(self.fd.as_raw_fd(), libc::TCIFLUSH) };
    }

    /// The port as a terminal for a person: 115200 baud, lines, echo, and
    /// the control characters it had.  It is left so.
    fn for_shell(&mut self) {
        let mut t = self.saved;
        t.c_iflag = libc::ICRNL | libc::IXON | libc::BRKINT | libc::IMAXBEL;
        t.c_oflag = libc::OPOST | libc::ONLCR;
        t.c_cflag = libc::CS8 | libc::CREAD | libc::CLOCAL;
        t.c_lflag =
            libc::ISIG | libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::IEXTEN | libc::ECHOCTL | libc::ECHOKE;
        // SAFETY: setting a termios on the open descriptor.
        unsafe {
            libc::cfsetispeed(&mut t, libc::B115200);
            libc::cfsetospeed(&mut t, libc::B115200);
            libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSANOW, &t);
        }
        self.saved = t;
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        // SAFETY: setting the saved termios back on the open descriptor.
        unsafe { libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSANOW, &self.saved) };
    }
}

/// Listens to both ports for the window; the port that asked for the shell.
fn window() -> Option<&'static str> {
    let mut ports: Vec<Port> = PORTS.iter().filter_map(|path| Port::open(path)).collect();
    if let Some(console) = ports.iter().find(|p| p.path == CONSOLE) {
        console.write(&format!("\r\nboot: Ctrl-R ({TRIGGER:#04x}) {RUN} times on this port or J508 for a shell\r\n"));
    }
    let end = Instant::now() + WINDOW;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || ports.is_empty() {
            return None;
        }
        let mut polls: Vec<libc::pollfd> =
            ports.iter().map(|p| libc::pollfd { fd: p.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: that many pollfds.
        let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, left.as_millis().max(1) as libc::c_int) };
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
        for (i, poll) in polls.iter().enumerate() {
            if poll.revents & libc::POLLIN != 0 && ports[i].read() {
                let port = &mut ports[i];
                port.drain();
                port.for_shell();
                // The shell opens the port itself.
                return Some(port.path);
            }
        }
    }
}

// -------------------------------------------------------------- the shell

/// The file systems the shell wants, as the recovery has them.
fn shell_system() {
    let _ = mount("proc", "/proc", "proc", 0, "");
    let _ = mount("sysfs", "/sys", "sysfs", 0, "");
    let _ = mount("tmpfs", "/tmp", "tmpfs", 0, "");
}

fn shell_system_away() {
    for target in ["/tmp", "/sys", "/proc"] {
        let _ = umount(target, libc::MNT_DETACH);
    }
}

/// A root shell on `tty`, until it ends.  Process 1 waits for every orphan
/// meanwhile.
fn shell(tty: &str) {
    shell_system();
    say(&format!("a shell on {tty}; the system starts when it ends ('reboot' restarts)"));
    let name = cstring(tty);
    let mut command = std::process::Command::new(SHELL);
    command.env_clear().envs([("PATH", "/bin"), ("HOME", "/tmp"), ("TERM", "vt100"), ("SHELL", SHELL)]).current_dir("/");
    // SAFETY: only async-signal-safe calls between fork and exec: a session
    // of its own, with the port as its terminal.
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            let fd = libc::open(name.as_ptr(), libc::O_RDWR);
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
    match command.spawn() {
        Ok(child) => loop {
            let mut status = 0;
            // SAFETY: a status to fill.
            let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
            if pid == child.id() as libc::pid_t || pid < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                break;
            }
        },
        Err(e) => {
            say(&format!("{SHELL} on {tty}: {e}"));
            std::thread::sleep(Duration::from_secs(5));
        }
    }
    shell_system_away();
}

// ------------------------------------------------------------ the system

/// What the command line says of the root and its init.
struct Root {
    device: String,
    kind: Option<String>,
    read_only: bool,
    init: Option<String>,
}

fn root(cmdline: &str) -> Option<Root> {
    let mut root = Root { device: String::new(), kind: None, read_only: false, init: None };
    for word in cmdline.split_ascii_whitespace() {
        match word.split_once('=') {
            Some(("root", v)) => root.device = v.to_string(),
            Some(("rootfstype", v)) => root.kind = Some(v.to_string()),
            Some(("init", v)) => root.init = Some(v.to_string()),
            None if word == "ro" => root.read_only = true,
            None if word == "rw" => root.read_only = false,
            _ => {}
        }
    }
    (!root.device.is_empty()).then_some(root)
}

/// Removes the ramdisk's files below `dir`, and nothing on another file
/// system.
fn remove_tree(dir: &Path, device: u64) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.dev() != device {
            continue;
        }
        if meta.is_dir() {
            remove_tree(&path, device);
            let _ = std::fs::remove_dir(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Mounts the root and runs its init; returns only when the root does not
/// mount or has no init.
fn system() -> String {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let _ = umount("/proc", libc::MNT_DETACH);
    let Some(root) = root(&cmdline) else { return "the command line names no root".into() };
    let flags = libc::MS_NOATIME | if root.read_only { libc::MS_RDONLY } else { 0 };
    let start = Instant::now();
    while !Path::new(&root.device).exists() && start.elapsed() < ROOT_WAIT {
        std::thread::sleep(Duration::from_millis(50));
    }
    let kinds: &[&str] = &match &root.kind {
        Some(kind) => kind.split(',').collect(),
        None => vec!["squashfs", "ext4"],
    };
    if !Path::new(&root.device).exists() {
        return format!("{} has not appeared", root.device);
    }
    let mut errors = Vec::new();
    for kind in kinds {
        match mount(&root.device, NEW_ROOT, kind, flags, "") {
            Ok(()) => break,
            Err(e) => errors.push(format!("as {kind}: {e}")),
        }
    }
    if errors.len() == kinds.len() {
        return format!("{} does not mount: {}", root.device, errors.join("; "));
    }
    let inits: Vec<&str> = match &root.init {
        Some(init) => vec![init.as_str()],
        None => INITS.to_vec(),
    };
    let in_root = |init: &str| format!("{NEW_ROOT}{init}");
    let Some(init) = inits.iter().find(|init| {
        let path = cstring(&in_root(init));
        // SAFETY: a NUL-terminated path.
        unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
    }) else {
        let _ = umount(NEW_ROOT, 0);
        return format!("{} has no init", root.device);
    };

    // From here on there is no way back into the ramdisk.
    if mount("/dev", &format!("{NEW_ROOT}/dev"), "", libc::MS_MOVE, "").is_err() {
        // The system mounts its own.
        let _ = umount("/dev", libc::MNT_DETACH);
    }
    let _ = umount("/sys", libc::MNT_DETACH);
    let _ = umount("/tmp", libc::MNT_DETACH);
    if let Ok(meta) = std::fs::symlink_metadata("/") {
        remove_tree(Path::new("/"), meta.dev());
    }
    let dot = cstring(".");
    // SAFETY: NUL-terminated paths: the new root becomes `/`, as
    // switch_root does.
    let moved = std::env::set_current_dir(NEW_ROOT).is_ok()
        && mount(".", "/", "", libc::MS_MOVE, "").is_ok()
        && unsafe { libc::chroot(dot.as_ptr()) } == 0
        && std::env::set_current_dir("/").is_ok();
    if !moved {
        say(&format!("cannot make {} the root: {}; restarting", root.device, std::io::Error::last_os_error()));
        std::thread::sleep(Duration::from_secs(5));
        restart();
    }
    // The kernel's environment, as the kernel gives it to init.
    let e = std::process::Command::new(init).arg0(init).exec();
    say(&format!("{init}: {e}; restarting"));
    std::thread::sleep(Duration::from_secs(5));
    restart()
}

// ---------------------------------------------------- the shell's commands

/// `mount`: the mounts; or `mount [-t TYPE] [-o OPTIONS] SOURCE TARGET`,
/// with `ro`, `rw`, `remount`, `bind`, `move`, `nosuid`, `nodev`, `noexec`,
/// `noatime`, and the rest as the file system's data.
fn mount_command(args: &[String]) -> i32 {
    if args.is_empty() {
        return match std::fs::read_to_string("/proc/mounts") {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(e) => {
                eprintln!("mount: /proc/mounts: {e}");
                1
            }
        };
    }
    let (mut kind, mut flags, mut data, mut rest) = (String::new(), 0, Vec::new(), Vec::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" => kind = args.next().cloned().unwrap_or_default(),
            "-r" => flags |= libc::MS_RDONLY,
            "-o" => {
                for option in args.next().map(String::as_str).unwrap_or_default().split(',') {
                    match option {
                        "ro" => flags |= libc::MS_RDONLY,
                        "rw" => flags &= !libc::MS_RDONLY,
                        "remount" => flags |= libc::MS_REMOUNT,
                        "bind" => flags |= libc::MS_BIND,
                        "move" => flags |= libc::MS_MOVE,
                        "nosuid" => flags |= libc::MS_NOSUID,
                        "nodev" => flags |= libc::MS_NODEV,
                        "noexec" => flags |= libc::MS_NOEXEC,
                        "noatime" => flags |= libc::MS_NOATIME,
                        "" | "defaults" => {}
                        other => data.push(other),
                    }
                }
            }
            _ => rest.push(arg.as_str()),
        }
    }
    let [source, target] = rest[..] else {
        eprintln!("usage: mount [-t TYPE] [-o OPTIONS] SOURCE TARGET");
        return 2;
    };
    let kinds: Vec<&str> = if kind.is_empty() && flags & (libc::MS_BIND | libc::MS_MOVE | libc::MS_REMOUNT) == 0 {
        vec!["ext4", "squashfs", "vfat"]
    } else {
        vec![kind.as_str()]
    };
    let mut last = None;
    for kind in kinds {
        let (source, target, kind_c, data) = (cstring(source), cstring(target), cstring(kind), cstring(&data.join(",")));
        let data = if data.is_empty() { std::ptr::null() } else { data.as_ptr().cast() };
        // SAFETY: NUL-terminated strings, and data that is text or nothing.
        match os(unsafe { libc::mount(source.as_ptr(), target.as_ptr(), kind_c.as_ptr(), flags, data) }) {
            Ok(()) => return 0,
            Err(e) => last = Some(e),
        }
    }
    eprintln!("mount: {target}: {}", last.map(|e| e.to_string()).unwrap_or_default());
    1
}

/// `umount [-l] TARGET...`
fn umount_command(args: &[String]) -> i32 {
    let lazy = args.iter().any(|a| a == "-l");
    let mut status = 0;
    for target in args.iter().filter(|a| !a.starts_with('-')) {
        if let Err(e) = umount(target, if lazy { libc::MNT_DETACH } else { 0 }) {
            eprintln!("umount: {target}: {e}");
            status = 1;
        }
    }
    status
}

fn main() {
    let mut args = std::env::args();
    let name = args.next().unwrap_or_default();
    let args: Vec<String> = args.collect();
    match name.rsplit('/').next() {
        Some("reboot") => restart(),
        Some("mount") => std::process::exit(mount_command(&args)),
        Some("umount") => std::process::exit(umount_command(&args)),
        _ => {}
    }
    if std::process::id() != 1 {
        eprintln!("boot-init: this is the init of the D80's boot image, and does nothing when it is not process 1");
        std::process::exit(2);
    }
    devices();
    if let Some(tty) = window() {
        shell(tty);
    }
    loop {
        let _ = mount("proc", "/proc", "proc", 0, "");
        let why = system();
        say(&why);
        shell(CONSOLE);
    }
}

#[cfg(test)]
mod tests {
    use super::root;

    #[test]
    fn reads_the_root_from_the_command_line() {
        let r = root("console=ttymxc0,115200 root=/dev/mmcblk0p5 rootfstype=squashfs ro rootwait vt.global_cursor_default=0").unwrap();
        assert_eq!(r.device, "/dev/mmcblk0p5");
        assert_eq!(r.kind.as_deref(), Some("squashfs"));
        assert!(r.read_only);
        assert!(r.init.is_none());
        let r = root("root=/dev/sda1 ro rw init=/bin/sh").unwrap();
        assert!(!r.read_only);
        assert_eq!(r.init.as_deref(), Some("/bin/sh"));
        assert!(root("console=ttymxc0").is_none());
    }
}
