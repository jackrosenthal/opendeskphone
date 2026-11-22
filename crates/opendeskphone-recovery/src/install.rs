//! Installing a package, the way Android's recovery does (`install.cpp`),
//! minus the signature check: the recovery installs whatever package is
//! left for it.  Packages still carry a signature, which identifies the
//! key they were built with and detects corruption, but nothing refuses a
//! package over it.
//!
//! A package brings its installer, `META-INF/com/google/android/update-binary`.
//! The recovery copies it out, and runs it with three arguments: the
//! version of the interface between them (3), the number of a pipe to the
//! recovery, and the package's path.  The program reads the package's
//! script and writes the partitions.  On the pipe it sends lines to the
//! recovery:
//!
//! | Line | Meaning |
//! | --- | --- |
//! | `progress FRACTION SECONDS` | The next part of the work is `FRACTION` of the bar; move through it over `SECONDS`, or as `set_progress` says when that is 0. |
//! | `set_progress FRACTION` | How far the current part is, 0 to 1. |
//! | `ui_print TEXT` | Text for the screen. |
//! | `wipe_cache` | Erase `/cache` when the package is installed. |
//! | `clear_display` | Clear the screen. |
//!
//! `wipe_cache` is not done here: `/cache` is `dphone`'s nvdata, and no
//! package of ours asks for it.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

/// The installer in a package.
pub const UPDATE_BINARY: &str = "META-INF/com/google/android/update-binary";

/// Where lines for the log go: the recovery's own, and the output of the
/// package's program.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// What an installation shows.
pub trait Ui {
    /// Text for the person at the phone.
    fn print(&mut self, text: &str);
    /// The bar is at `position`, 0 to 1.
    fn progress(&mut self, position: f32);
}

/// The bar, as Android's recovery moves it: in parts, each a fraction of
/// the whole, through which it goes either with time or as it is told.
#[derive(Debug)]
pub struct Bar {
    start: f32,
    size: f32,
    fraction: f32,
    seconds: f32,
    since: Instant,
}

impl Default for Bar {
    fn default() -> Self {
        Bar { start: 0.0, size: 0.0, fraction: 0.0, seconds: 0.0, since: Instant::now() }
    }
}

impl Bar {
    /// The next part: `portion` of the whole, after the parts so far.
    pub fn show(&mut self, portion: f32, seconds: f32, now: Instant) {
        self.start += self.size;
        self.size = portion;
        self.fraction = 0.0;
        self.seconds = seconds;
        self.since = now;
    }

    /// How far the current part is.
    pub fn set(&mut self, fraction: f32) {
        self.fraction = fraction.clamp(0.0, 1.0);
    }

    pub fn position(&self, now: Instant) -> f32 {
        let fraction = if self.seconds > 0.0 {
            (now.saturating_duration_since(self.since).as_secs_f32() / self.seconds).min(1.0).max(self.fraction)
        } else {
            self.fraction
        };
        (self.start + self.size * fraction).clamp(0.0, 1.0)
    }
}

/// A pipe whose ends are closed in programs that are started, unless one
/// is handed over on purpose.
fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors, which are then owned
    // here and nowhere else.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

/// Installs the package at `package`.  `tmp` is a directory in memory,
/// for the package's program.
pub fn install(package: &Path, tmp: &Path, ui: &mut dyn Ui, log: &Log) -> Result<(), String> {
    let name = package.display();
    let mut bar = Bar::default();
    ui.print("Finding update package...");
    let file = File::open(package).map_err(|e| format!("{name}: {e}"))?;
    let len = file.metadata().map_err(|e| format!("{name}: {e}"))?.len();
    log(&format!("update package: {name}, {len} bytes"));

    ui.print("Installing update...");
    let mut archive = crate::zip::Archive::open(file, len)?;
    let program = tmp.join("update_binary");
    let _ = std::fs::remove_file(&program);
    std::fs::write(&program, archive.read(UPDATE_BINARY, 16 << 20)?).map_err(|e| format!("{}: {e}", program.display()))?;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", program.display()))?;
    drop(archive);

    let (from_program, to_recovery) = pipe().map_err(|e| format!("a pipe: {e}"))?;
    let (output, program_output) = pipe().map_err(|e| format!("a pipe: {e}"))?;
    let fd = to_recovery.as_raw_fd();
    let mut command = Command::new(&program);
    command.arg("3").arg(fd.to_string()).arg(package);
    command.env_clear().env("PATH", "/sbin:/bin").current_dir("/");
    command.stdin(Stdio::null());
    command.stdout(program_output.try_clone().map_err(|e| format!("a pipe: {e}"))?).stderr(program_output);
    // SAFETY: `fcntl` is all that runs between fork and exec; it leaves
    // the pipe's end open in the program.
    unsafe {
        command.pre_exec(move || if libc::fcntl(fd, libc::F_SETFD, 0) == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) });
    }
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", program.display()))?;
    // The program's ends of the pipes, which `command` held.
    drop(command);
    drop(to_recovery);

    // What the program prints, to the log.
    let printed = {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut output = BufReader::new(File::from(output));
            while output.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                log(String::from_utf8_lossy(&line).trim_end());
                line.clear();
            }
        })
    };

    // The bar moves with time between the program's lines too.
    let lines = {
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut pipe = BufReader::new(File::from(from_program));
            while pipe.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                if send.send(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']).to_string()).is_err() {
                    break;
                }
                line.clear();
            }
        });
        lines
    };
    loop {
        match lines.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => {
                let (word, rest) = line.split_once(' ').unwrap_or((&line, ""));
                let mut numbers = rest.split_ascii_whitespace().map(|n| n.parse::<f32>().unwrap_or(0.0));
                match word {
                    "progress" => {
                        let (portion, seconds) = (numbers.next().unwrap_or(0.0), numbers.next().unwrap_or(0.0));
                        bar.show(portion, seconds, Instant::now());
                    }
                    "set_progress" => bar.set(numbers.next().unwrap_or(0.0)),
                    "ui_print" => ui.print(rest),
                    "wipe_cache" => log("the package asks for /cache to be erased, which this recovery does not do"),
                    "clear_display" => {}
                    other => log(&format!("unknown command from the package's program: {other}")),
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        ui.progress(bar.position(Instant::now()));
    }
    let status = child.wait().map_err(|e| format!("{}: {e}", program.display()))?;
    let _ = printed.join();
    let _ = std::io::stdout().flush();
    if !status.success() {
        return Err(format!("the package's program failed ({status})"));
    }
    bar.set(1.0);
    ui.progress(1.0);
    Ok(())
}

/// Reads all of a file that may not be there.
pub fn read_optional(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::FileEntry;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen {
        printed: Vec<String>,
        positions: Vec<f32>,
    }

    impl Ui for Seen {
        fn print(&mut self, text: &str) {
            self.printed.push(text.to_string());
        }
        fn progress(&mut self, position: f32) {
            self.positions.push(position);
        }
    }

    /// A package whose installer is a shell script.  The scripts use
    /// nothing but the shell: a build sandbox has little else.
    fn package(dir: &Path, script: &str) -> std::path::PathBuf {
        let zip = crate::zip::write(&[
            FileEntry { name: UPDATE_BINARY, data: script.as_bytes(), executable: true },
            FileEntry { name: "payload", data: b"the payload\n", executable: false },
        ])
        .unwrap();
        let path = dir.join("update.zip");
        std::fs::write(&path, zip).unwrap();
        path
    }

    fn run(script: &str) -> (Result<(), String>, Seen, Vec<String>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let package = package(dir.path(), script);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log: Log = {
            let lines = lines.clone();
            Arc::new(move |line: &str| lines.lock().unwrap().push(line.to_string()))
        };
        let mut seen = Seen::default();
        let result = install(&package, dir.path(), &mut seen, &log);
        let lines = lines.lock().unwrap().clone();
        (result, seen, lines, dir)
    }

    #[test]
    fn the_packages_program_is_run_and_heard() {
        let script = "#!/bin/sh\n\
            test \"$1\" = 3 || exit 9\n\
            echo \"progress 0.800000 0\" >&$2\n\
            echo \"ui_print Open Desk Phone\" >&$2\n\
            echo \"set_progress 0.500000\" >&$2\n\
            echo \"said on stdout: $3\"\n\
            echo \"said on stderr\" >&2\n\
            echo \"$3\" > \"${3%/*}/ran\"\n\
            echo \"progress 0.200000 0\" >&$2\n\
            echo \"set_progress 1.000000\" >&$2\n";
        let (result, seen, log, dir) = run(script);
        assert_eq!(result, Ok(()));
        let package = dir.path().join("update.zip");
        assert_eq!(std::fs::read_to_string(dir.path().join("ran")).unwrap().trim(), package.to_str().unwrap());
        assert!(seen.printed.iter().any(|l| l == "Open Desk Phone"), "{:?}", seen.printed);
        // The bar never goes back, and ends full: the program's parts are
        // shares of the whole.
        assert!(seen.positions.windows(2).all(|w| w[0] <= w[1]), "{:?}", seen.positions);
        assert_eq!(seen.positions.last(), Some(&1.0));
        assert!(seen.positions.iter().any(|p| (p - 0.8).abs() < 1e-6), "{:?}", seen.positions);
        assert!(seen.positions.iter().any(|p| (p - 0.8 * 0.5).abs() < 1e-6), "{:?}", seen.positions);
        assert!(log.iter().any(|l| l == &format!("said on stdout: {}", package.display())), "{log:?}");
        assert!(log.iter().any(|l| l == "said on stderr"));
    }

    #[test]
    fn a_failing_program_fails_the_installation() {
        let (result, _, log, _dir) = run("#!/bin/sh\necho no good\nexit 7\n");
        assert!(result.unwrap_err().contains("failed"));
        assert!(log.iter().any(|l| l == "no good"));
    }

    #[test]
    fn the_bar_goes_through_its_parts() {
        let t = Instant::now();
        let mut bar = Bar::default();
        bar.show(0.25, 0.0, t);
        bar.set(0.5);
        assert_eq!(bar.position(t), 0.125);
        // A part with a time moves by itself, and not past its end.
        bar.show(0.5, 10.0, t);
        assert_eq!(bar.position(t), 0.25);
        assert_eq!(bar.position(t + std::time::Duration::from_secs(5)), 0.5);
        assert_eq!(bar.position(t + std::time::Duration::from_secs(60)), 0.75);
        bar.show(0.25, 0.0, t);
        bar.set(1.0);
        assert_eq!(bar.position(t), 1.0);
    }
}
