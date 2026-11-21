//! End-to-end tests on the build host: the real binary, in simulator mode.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_dphone");

/// A temp directory whose paths are short enough for a Unix socket.
fn temp() -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("dp").tempdir().unwrap();
    if dir.path().as_os_str().len() < 60 {
        return dir;
    }
    tempfile::Builder::new().prefix("dp").tempdir_in("/tmp").unwrap()
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The first child process of `parent`, from `/proc`.  (Not `pgrep`: the
/// test must not depend on which tools the machine running it has.)
fn child_of(parent: u32) -> Option<i32> {
    let mut children: Vec<i32> = std::fs::read_dir("/proc")
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            // "PID (COMM) STATE PPID ..."; COMM may itself hold spaces and
            // parentheses, so the fields are counted from its last ')'.
            std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().is_some_and(|stat| {
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_ascii_whitespace().nth(1)?.parse::<u32>().ok())
                    == Some(parent)
            })
        })
        .collect();
    children.sort_unstable();
    children.first().copied()
}

/// Prints a file when the test that holds it fails.
struct ShowOnPanic(PathBuf);

impl Drop for ShowOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("--- {} ---\n{}", self.0.display(), std::fs::read_to_string(&self.0).unwrap_or_default());
        }
    }
}

/// A child started with `process_group(0)`, ended with everything it started
/// when the test is over, also when the test fails: a supervisor that is
/// killed cannot stop its main role, which would otherwise live on (and
/// keep the test run's output pipe open).
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // SAFETY: plain kill(2) on the process group our child leads; the
        // child is not reaped yet, so the group ID cannot have been reused.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn diag(socket: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN).args(args).env("DPHONE_DIAG_SOCKET", socket).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn start_main(dir: &Path, extra: &[&str]) -> (KillOnDrop, PathBuf) {
    start_main_as(dir, "d40", extra)
}

fn start_main_as(dir: &Path, model: &str, extra: &[&str]) -> (KillOnDrop, PathBuf) {
    let nv = dir.join("nv");
    let run = dir.join("run");
    std::fs::create_dir_all(&nv).unwrap();
    let child = Command::new(BIN)
        .args(["main", "--sim", "--model", model, "--nvdata"])
        .arg(&nv)
        .arg("--run-dir")
        .arg(&run)
        .args(extra)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let socket = run.join("diag.sock");
    (KillOnDrop(child), socket)
}

#[test]
fn diag_commands_and_restart() {
    let dir = temp();
    let (mut main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());

    let (code, out, _) = diag(&socket, &["ping"]);
    assert_eq!((code, out.as_str()), (0, "pong\n"));

    let (code, out, _) = diag(&socket, &["version"]);
    assert_eq!(code, 0);
    assert!(out.contains("Open Desk Phone") && out.contains("model D40"), "{out}");

    // The subsystems wired into the main role answer.
    let (code, out, _) = diag(&socket, &["state", "system"]);
    assert_eq!(code, 0);
    assert!(out.contains("D40") && out.contains("simulated: true"), "{out}");
    let (code, out, _) = diag(&socket, &["config", "get", "network_enable_dhcp"]);
    assert_eq!(code, 0, "{out}");
    let (code, out, _) = diag(&socket, &["hal", "status"]);
    assert_eq!(code, 0);
    assert!(out.contains("simulator") && out.contains("240x120"), "{out}");
    let (code, out, _) = diag(&socket, &["audio"]);
    assert_eq!(code, 0);
    assert!(out.contains("engine: running"), "{out}");
    let (code, out, _) = diag(&socket, &["app", "list"]);
    assert_eq!(code, 0, "{out}");
    let (code, out, _) = diag(&socket, &["net"]);
    assert_eq!(code, 0);
    assert!(out.contains("simulator"), "{out}");

    // Keys are checked before any is pressed.
    let (code, _, err) = diag(&socket, &["key", "1", "nope"]);
    assert_eq!(code, 2);
    assert!(err.contains("invalid value 'nope'"), "{err}");
    let (code, _, err) = diag(&socket, &["key", "1", "2", "#"]);
    assert_eq!(code, 0, "{err}");
    // A D40 has no touchscreen.
    let (code, _, err) = diag(&socket, &["touch", "10", "10"]);
    assert_eq!(code, 1);
    assert!(err.contains("the D40 has no touchscreen"), "{err}");

    wait_for("healthy", Duration::from_secs(10), || diag(&socket, &["health"]).0 == 0);
    let (code, out, _) = diag(&socket, &["log", "-n", "50"]);
    assert_eq!(code, 0);
    assert!(out.contains("main role starting"), "{out}");

    let (code, _, _) = diag(&socket, &["restart"]);
    assert_eq!(code, 0);
    let status = main.0.wait().unwrap();
    assert_eq!(status.code(), Some(0));
}

/// A phone with no saved server and no accounts (the state after a factory
/// reset) shows the server menu, not the idle screen, until the user goes
/// on without a server or the phone is provisioned.
#[test]
fn a_phone_with_no_server_and_no_accounts_shows_the_server_menu() {
    let dir = temp();
    // A SIP port of the phone's own: the DPMA client starts once the SIP
    // stack has its port, and the other tests of this file, which run at
    // the same time, each have a simulator that asks for 5060.
    let sip_port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::create_dir_all(dir.path().join("nv")).unwrap();
    std::fs::write(
        dir.path().join("nv/ui-config.xml"),
        format!("<config>\n    <setting id=\"transport_udp_port\" value=\"{sip_port}\" />\n</config>\n"),
    )
    .unwrap();
    let (_main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    let state = |topic: &str| diag(&socket, &["state", topic]).1;
    wait_for("the server menu", Duration::from_secs(30), || state("ui").contains("screen: \"provisioning\""));
    assert!(state("system").contains("phase: Provisioning"), "{}", state("system"));
    // Setup shows from the end of start-up; the client then starts and
    // waits for a server.
    wait_for("the client waiting for a server", Duration::from_secs(30), || state("provisioning").contains("phase: AwaitingServer"));
    // The rows are "Enter address..." and "Use without a server".
    assert_eq!(diag(&socket, &["key", "down", "select"]).0, 0);
    wait_for("the idle screen", Duration::from_secs(10), || state("ui").contains("screen: \"idle\""));
    assert!(state("system").contains("phase: Running"), "{}", state("system"));
    assert!(state("provisioning").contains("phase: Standalone"), "{}", state("provisioning"));
}

/// The D80 is operated by touch: `dphone touch` taps the server menu's
/// second row, and a place off the screen is refused.
#[test]
fn a_d80_is_operated_by_touch() {
    let dir = temp();
    // A SIP port of the phone's own (see the test above).
    let sip_port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::create_dir_all(dir.path().join("nv")).unwrap();
    std::fs::write(
        dir.path().join("nv/ui-config.xml"),
        format!("<config>\n    <setting id=\"transport_udp_port\" value=\"{sip_port}\" />\n</config>\n"),
    )
    .unwrap();
    let (_main, socket) = start_main_as(dir.path(), "d80", &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    let state = |topic: &str| diag(&socket, &["state", topic]).1;
    wait_for("the server menu", Duration::from_secs(30), || state("ui").contains("screen: \"provisioning\""));
    wait_for("the client waiting for a server", Duration::from_secs(30), || state("provisioning").contains("phase: AwaitingServer"));

    let (code, _, err) = diag(&socket, &["touch", "800", "10"]);
    assert_eq!(code, 1);
    assert!(err.contains("(800, 10) is not on the 800x1280 screen"), "{err}");
    let (code, _, err) = diag(&socket, &["touch", "10", "10", "--to", "10", "1280"]);
    assert_eq!(code, 1);
    assert!(err.contains("(10, 1280) is not on the 800x1280 screen"), "{err}");
    assert!(state("ui").contains("screen: \"provisioning\""), "{}", state("ui"));

    // "Use without a server" is the left one of the two buttons across the
    // bottom: 96 pixels high, 12 above the bottom edge, from x 20 to 392.
    assert_eq!(diag(&socket, &["touch", "200", "1220"]).0, 0);
    wait_for("the idle screen", Duration::from_secs(10), || state("ui").contains("screen: \"idle\""));
    assert!(state("provisioning").contains("phase: Standalone"), "{}", state("provisioning"));
    // A drag is taken too.
    let (code, _, err) = diag(&socket, &["touch", "400", "1000", "--to", "400", "300", "--steps", "5"]);
    assert_eq!(code, 0, "{err}");
}

/// A factory reset from the phone's own menu, on a provisioned phone: the
/// menu writes stock's marker and the phone restarts; the next start
/// performs the reset and comes up at the server menu, not the idle screen.
#[test]
fn a_factory_reset_from_the_menu_leads_to_the_server_menu() {
    let dir = temp();
    let nv = dir.path().join("nv");
    let put = |rel: &str, text: &str| {
        let path = nv.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    // A phone a server provisioned: the saved server (nothing answers
    // there), the configuration it sent with an account, the phone's own
    // settings, and state of ours under open/.  The account shows once the
    // SIP stack has bound its port, so the phone has a SIP port of its own:
    // 5060 may be held by the other tests of this file, which run at the
    // same time, or by a SIP server on the machine.
    let sip_port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    put("ui-config.xml", &format!("<config>\n    <setting id=\"transport_udp_port\" value=\"{sip_port}\" />\n</config>\n"));
    put("configurator.xml", "<config>\n    <setting id=\"config_server_url\" value=\"sip:proxy@127.0.0.1:9\" />\n</config>\n");
    put(
        "fetched-config.xml",
        "<config>\n<accounts>\n<account index=\"0\" account_id=\"100\" username=\"100\" password=\"x\" authname=\"100\" line_label=\"100\" register=\"1\" type=\"sip\">\n<host_primary server=\"127.0.0.1\" port=\"9\" transport=\"udp\" />\n</account>\n</accounts>\n<setting id=\"opendeskphone_ssh_enabled\" value=\"1\" />\n</config>\n",
    );
    put("opendeskphone/settings.txtpb", "serial_console: true\n");
    put("opendeskphone/net-no-bridge", "1\n");
    put("systime", "2026-11-15 04:42:00\n");

    let state = |socket: &Path, topic: &str| diag(socket, &["state", topic]).1;
    let screen = |socket: &Path| {
        let ui = state(socket, "ui");
        ui.lines().find(|l| l.trim_start().starts_with("screen:")).unwrap_or_default().trim().to_string()
    };
    let (mut main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("the idle screen of a provisioned phone", Duration::from_secs(30), || screen(&socket) == "screen: \"idle\",");
    assert!(state(&socket, "system").contains("phase: Running"), "{}", state(&socket, "system"));
    wait_for("the provisioned account", Duration::from_secs(30), || state(&socket, "accounts").contains("\"100\""));

    // Menu, Settings, Reset, Factory reset, Yes.
    let press = |keys: &[&str], want: &str| {
        let mut args = vec!["key"];
        args.extend_from_slice(keys);
        assert_eq!(diag(&socket, &args).0, 0, "{keys:?}");
        wait_for(want, Duration::from_secs(10), || screen(&socket) == format!("screen: \"{want}\","));
    };
    press(&["menu"], "menu");
    press(&["down", "down", "down", "down", "down", "select"], "settings");
    press(&["down", "down", "down", "down", "select"], "reset");
    press(&["down", "down", "down", "select"], "confirm");
    assert_eq!(diag(&socket, &["key", "select"]).0, 0);
    // The marker is written and the main role goes down for the restart.
    let status = main.0.wait().unwrap();
    assert!(nv.join("factory-reset").exists(), "the marker (exit {status:?})");
    assert!(nv.join("configurator.xml").exists(), "the reset itself is for the next start");

    // The next start.
    let (_main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket again", Duration::from_secs(10), || socket.exists() && diag(&socket, &["ping"]).0 == 0);
    wait_for("the server menu after the reset", Duration::from_secs(30), || screen(&socket) == "screen: \"provisioning\",");
    for gone in ["factory-reset", "configurator.xml", "fetched-config.xml", "opendeskphone/settings.txtpb", "opendeskphone/net-no-bridge", "systime"] {
        assert!(!nv.join(gone).exists(), "{gone} survived the reset");
    }
    assert!(state(&socket, "system").contains("phase: Provisioning"), "{}", state(&socket, "system"));
    assert!(state(&socket, "provisioning").contains("server: None"), "{}", state(&socket, "provisioning"));
    // Setup was decided from nvdata when start-up ended, not left to the
    // DPMA client, which on a phone starts only once there is an address.
    let log = diag(&socket, &["log", "-n", "500"]).1;
    assert!(log.contains("the phone starts at the server menu"), "{log}");
    assert!(!state(&socket, "accounts").contains("\"100\""), "{}", state(&socket, "accounts"));
}

/// The speaker, headset, and mute lamps follow the audio state the main
/// role keeps.
#[test]
fn audio_lamps_follow_the_audio_state() {
    let dir = temp();
    let (_main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("audio engine", Duration::from_secs(10), || diag(&socket, &["audio"]).1.contains("engine: running"));
    // The three lamps as the state store has them.
    let lamps = || {
        let keys = diag(&socket, &["state", "keys"]).1;
        let field = |name: &str| {
            let rest = keys.split_once(name).unwrap_or_else(|| panic!("no {name} in: {keys}")).1;
            let color = rest.split_once("color: ").unwrap().1;
            color.split(|c: char| !c.is_alphanumeric()).next().unwrap().to_string()
        };
        [field("speaker_lamp"), field("headset_lamp"), field("mute_lamp")]
    };
    let run = |args: &[&str]| assert_eq!(diag(&socket, args).0, 0, "{args:?}");
    assert_eq!(lamps(), ["Off", "Off", "Off"]);

    // On-hook dialing: the speaker path with dial tone.
    run(&["audio", "path", "speaker"]);
    run(&["audio", "tone", "dial"]);
    wait_for("speaker lamp", Duration::from_secs(5), || lamps() == ["Green", "Off", "Off"]);
    run(&["audio", "mute", "on"]);
    wait_for("mute lamp", Duration::from_secs(5), || lamps() == ["Green", "Off", "Red"]);
    run(&["audio", "path", "headset"]);
    wait_for("headset lamp", Duration::from_secs(5), || lamps() == ["Off", "Green", "Red"]);
    run(&["audio", "path", "handset"]);
    wait_for("handset", Duration::from_secs(5), || lamps() == ["Off", "Off", "Red"]);
    run(&["audio", "mute", "off"]);
    run(&["audio", "tone", "off"]);
    run(&["audio", "path", "none"]);
    wait_for("all off", Duration::from_secs(5), || lamps() == ["Off", "Off", "Off"]);
}

/// Run the binary with standard output and standard error both connected to
/// pipes that nobody reads, as in `dphone ... 2>&1 | head` after `head` has
/// gone.  Returns the exit status.
fn run_into_closed_pipes(socket: Option<&Path>, args: &[&str]) -> i32 {
    let closed = || {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        Stdio::from(writer)
    };
    let mut cmd = Command::new(BIN);
    cmd.args(args).stdin(Stdio::null()).stdout(closed()).stderr(closed());
    if let Some(socket) = socket {
        cmd.env("DPHONE_DIAG_SOCKET", socket);
    }
    // A panic exits with 101; death by a signal has no exit code.
    cmd.status().unwrap().code().expect("killed by a signal")
}

#[test]
fn output_into_a_closed_pipe_is_not_a_panic() {
    // Commands that run locally: standard output, then standard error.
    assert_eq!(run_into_closed_pipes(None, &["--help"]), 0);
    assert_eq!(run_into_closed_pipes(None, &["--version"]), 0);
    assert_eq!(run_into_closed_pipes(None, &["main", "--help"]), 0);
    assert_eq!(run_into_closed_pipes(None, &["mount"]), 0);
    assert_eq!(run_into_closed_pipes(None, &[]), 2);
    assert_eq!(run_into_closed_pipes(None, &["update"]), 2);
    assert_eq!(run_into_closed_pipes(None, &["update", "switch-bank", "c"]), 2);
    assert_eq!(run_into_closed_pipes(None, &["umount"]), 2);
    assert_eq!(run_into_closed_pipes(None, &["umount", "/no/such/mount/point"]), 1);
    assert_eq!(run_into_closed_pipes(None, &["main", "--no-such-option"]), 2);
    assert_eq!(run_into_closed_pipes(None, &["supervise", "--no-such-option"]), 2);

    // Commands relayed from the main role, and the main role not being there.
    let dir = temp();
    let (_main, socket) = start_main(dir.path(), &["--no-ssh"]);
    assert_eq!(run_into_closed_pipes(Some(&dir.path().join("none.sock")), &["ping"]), 1);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("main role", Duration::from_secs(10), || diag(&socket, &["ping"]).0 == 0);
    assert_eq!(run_into_closed_pipes(Some(&socket), &["state"]), 1);
    assert_eq!(run_into_closed_pipes(Some(&socket), &["help"]), 0);
    assert_eq!(run_into_closed_pipes(Some(&socket), &["no-such-command"]), 2);
    // Following the log would never end by itself.
    assert_eq!(run_into_closed_pipes(Some(&socket), &["log", "-f"]), 1);

    // The same when the reader goes away later, while the log is quiet
    // (`dphone log -f | head -1`): there is then no write to fail, so the
    // command has to notice the closed pipe by itself.
    wait_for("healthy", Duration::from_secs(10), || diag(&socket, &["health"]).0 == 0);
    let quiet = |socket: &Path| {
        let lines = |socket: &Path| diag(socket, &["log", "-n", "100000"]).1.lines().count();
        let before = lines(socket);
        std::thread::sleep(Duration::from_secs(1));
        lines(socket) == before
    };
    wait_for("a quiet log", Duration::from_secs(20), || quiet(&socket));
    let mut follow = Command::new(BIN)
        .args(["log", "-f"])
        .env("DPHONE_DIAG_SOCKET", &socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(follow.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(!first.is_empty());
    // The backlog is out; nothing more is coming.  Close the pipe.
    std::thread::sleep(Duration::from_millis(500));
    drop(reader);
    let closed = Instant::now();
    let mut follow = KillOnDrop(follow);
    wait_for("log -f to end after its output closed", Duration::from_secs(3), || follow.0.try_wait().unwrap().is_some());
    assert!(closed.elapsed() < Duration::from_secs(3));
    assert_eq!(follow.0.try_wait().unwrap().unwrap().code(), Some(1));
}

#[test]
fn supervisor_confirms_trial_and_restarts_main() {
    let dir = temp();
    let nv = dir.path().join("nv");
    let run = dir.path().join("run");
    std::fs::create_dir_all(nv.join("opendeskphone")).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    let trial = nv.join("opendeskphone/trial");
    std::fs::write(&trial, "pending=1\nprevious_bank=0\nboots=0\nmax_boots=3\n").unwrap();

    // What the supervisor and the main roles log, shown if the test fails.
    let log = dir.path().join("log");
    let _show_log = ShowOnPanic(log.clone());
    let sup = Command::new(BIN)
        .args(["supervise", "--nvdata"])
        .arg(&nv)
        .arg("--run-dir")
        .arg(&run)
        .args(["--grace", "20", "--beat-timeout", "3", "--"])
        .arg(BIN)
        .args(["main", "--sim", "--no-ssh", "--nvdata"])
        .arg(&nv)
        .arg("--run-dir")
        .arg(&run)
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    let sup = KillOnDrop(sup);
    let socket = run.join("diag.sock");

    // The main role reports healthy; the trial is confirmed.
    wait_for("trial confirmation", Duration::from_secs(15), || !trial.exists());

    let main_pid = |sup: &KillOnDrop| child_of(sup.0.id());
    wait_for("main role", Duration::from_secs(10), || diag(&socket, &["ping"]).0 == 0);
    let first = main_pid(&sup).expect("main role pid");

    // Kill it: the supervisor starts another.
    // SAFETY: plain kill(2) on a child of our child.
    unsafe { libc::kill(first, libc::SIGKILL) };
    wait_for("restart after kill", Duration::from_secs(15), || {
        main_pid(&sup).is_some_and(|p| p != first) && diag(&socket, &["ping"]).0 == 0
    });
    let second = main_pid(&sup).unwrap();

    // Hang it: the heartbeat stops, the supervisor kills and restarts it.
    // The short beat timeout applies only once the supervisor has had a beat
    // from this instance; one that has not beaten yet is given the whole
    // start grace.  The diagnostics socket answers before the first beat is
    // written, so "answers ping" is too early to stop it: wait until the
    // supervisor says it has heard from this instance (its second such line;
    // the first was for the instance that confirmed the trial).
    let heard = || std::fs::read_to_string(&log).unwrap_or_default().matches("main role reports healthy").count();
    wait_for("second instance healthy", Duration::from_secs(15), || heard() >= 2);
    // SAFETY: as above.
    unsafe { libc::kill(second, libc::SIGSTOP) };
    wait_for("restart after hang", Duration::from_secs(20), || {
        main_pid(&sup).is_some_and(|p| p != second) && diag(&socket, &["ping"]).0 == 0
    });

    // SIGTERM to the supervisor ("reboot"): it stops the main role and ends.
    let third = main_pid(&sup).unwrap();
    // SAFETY: as above.
    unsafe { libc::kill(sup.0.id() as i32, libc::SIGTERM) };
    wait_for("main role stopped", Duration::from_secs(10), || unsafe { libc::kill(third, 0) } != 0);
}

/// Start the supervisor alone, with `main` as the main role's command line
/// and a pending trial whose previous bank is 1.  Returns the supervisor,
/// the trial record's path, the rollback marker's path, and the log.
fn start_trial_supervisor(dir: &Path, main: &[&str]) -> (KillOnDrop, PathBuf, PathBuf, PathBuf) {
    let nv = dir.join("nv");
    let run = dir.join("run");
    std::fs::create_dir_all(nv.join("opendeskphone")).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    let trial = nv.join("opendeskphone/trial");
    std::fs::write(&trial, "pending=1\nprevious_bank=1\nboots=0\nmax_boots=3\n").unwrap();
    let marker = dir.join("rolled-back-to");
    let log = dir.join("log");
    let sup = Command::new(BIN)
        .args(["supervise", "--nvdata"])
        .arg(&nv)
        .arg("--run-dir")
        .arg(&run)
        .args(["--grace", "20", "--beat-timeout", "3", "--rollback-marker"])
        .arg(&marker)
        .arg("--")
        .arg(BIN)
        .args(main)
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    (KillOnDrop(sup), trial, marker, log)
}

/// An image whose main role fails at every start is not kept: during the
/// trial the full main role is retried (never the safe-mode one, which
/// would come up and look fine), and after `MAX_TRIAL_CRASHES` failures the
/// supervisor selects the previous bank and reboots.
#[test]
fn crash_looping_main_role_during_a_trial_rolls_back() {
    let dir = temp();
    // `dphone main` with an option it does not know exits with status 2.
    let (mut sup, trial, marker, log) = start_trial_supervisor(dir.path(), &["main", "--no-such-option"]);
    let _show_log = ShowOnPanic(log.clone());

    // Restart delays are 1, 2, 4, and 8 s before the fifth failure.
    wait_for("the supervisor to give up and reboot", Duration::from_secs(40), || sup.0.try_wait().unwrap().is_some());
    assert_eq!(sup.0.try_wait().unwrap().unwrap().code(), Some(0));

    assert_eq!(std::fs::read_to_string(&marker).expect("the rollback hook ran"), "1", "previous bank selected");
    assert!(!trial.exists(), "the trial record is gone after the rollback");
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text.matches("started main role").count(), 5, "{text}");
    assert!(!text.contains("safe mode"), "safe mode must not be tried during a trial:\n{text}");
    assert!(!text.contains("trial boot confirmed"), "{text}");
    assert!(text.contains("reverted to bank 1"), "{text}");
    let reason = std::fs::read_to_string(dir.path().join("nv/reboot_reason")).unwrap();
    assert_eq!(reason, "init: main role keeps failing during trial boot, reverted to bank 1\n");
}

/// A main role in safe mode comes up and says so, and that does not confirm
/// a trial boot.
#[test]
fn safe_mode_does_not_confirm_a_trial() {
    let dir = temp();
    let nv = dir.path().join("nv");
    let run = dir.path().join("run");
    let (nv_s, run_s) = (nv.to_str().unwrap().to_owned(), run.to_str().unwrap().to_owned());
    let (_sup, trial, marker, log) = start_trial_supervisor(
        dir.path(),
        &["main", "--sim", "--safe", "--no-ssh", "--nvdata", &nv_s, "--run-dir", &run_s],
    );
    let _show_log = ShowOnPanic(log.clone());
    let logged = |what: &str| std::fs::read_to_string(&log).unwrap_or_default().contains(what);
    // The supervisor says so when the beat that reports safe mode comes;
    // a beat that confirmed the trial would have come in its place.  (The
    // main role's own half of the confirmation does nothing in the
    // simulator.)
    wait_for("the safe-mode main role to report", Duration::from_secs(20), || logged("main role is up in safe mode"));
    assert!(trial.exists(), "the trial record must stay");
    assert!(!logged("trial boot confirmed") && !logged("main role reports healthy"));
    assert!(!marker.exists(), "nothing failed, so nothing is rolled back");
}

/// A loopback port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// One HTTP/1.1 exchange; returns the status and the body.
fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let status = raw.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, raw.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default())
}

/// With nothing set, the phone does not listen for SSH at any point; the
/// listener opens when `ssh_enabled` is turned on, and closes again.
#[test]
fn ssh_is_off_until_the_setting_turns_it_on() {
    let dir = temp();
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let (main, socket) = start_main(dir.path(), &["--ssh-listen", &listen]);
    let _main = main;
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("healthy", Duration::from_secs(10), || diag(&socket, &["health"]).0 == 0);
    let log = || diag(&socket, &["log", "-n", "100000"]).1;
    assert!(!listening(port), "SSH listens although nothing turned it on");
    assert!(!log().contains("ssh listening"), "{}", log());

    let settings = dir.path().join("nv/opendeskphone/settings.txtpb");
    std::fs::write(&settings, "ssh_enabled: true\n").unwrap();
    assert_eq!(diag(&socket, &["config", "reload"]).0, 0);
    wait_for("the SSH listener", Duration::from_secs(10), || listening(port));

    std::fs::write(&settings, "ssh_enabled: false\n").unwrap();
    assert_eq!(diag(&socket, &["config", "reload"]).0, 0);
    wait_for("the SSH listener to close", Duration::from_secs(10), || !listening(port));
}

/// The web server listens whatever `web_ui_enabled` and `metrics_enabled`
/// say, and follows both while the main role runs.
#[cfg(feature = "web")]
#[test]
fn web_configuration_and_metrics_switch_at_run_time() {
    let dir = temp();
    let port = free_port();
    let (main, socket) = start_main(dir.path(), &["--no-ssh", "--web-port", &port.to_string()]);
    let _main = main;
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("the web server", Duration::from_secs(20), || listening(port));
    let session = || http(port, "POST", "/opendeskphone.v1.AuthService/GetSession", "{}");
    let login = || http(port, "POST", "/opendeskphone.v1.AuthService/Login", r#"{"user":"admin","password":"789"}"#).0;
    let shell = || http(port, "POST", "/opendeskphone.v1.ShellService/OpenShell", "{}").0;
    let metrics = || http(port, "GET", "/metrics", "").0;
    let set = |id: &str, value: &str| assert_eq!(diag(&socket, &["config", "set", id, value]).0, 0);

    // Defaults: web configuration on, metrics on.
    assert!(session().1.contains("\"model\""), "{:?}", session());
    assert_eq!((login(), metrics()), (200, 200));

    // Web configuration off: a page, the state, metrics, and nothing else.
    set("web_ui_enabled", "0");
    wait_for("web configuration to be disabled", Duration::from_secs(10), || login() == 403);
    let (status, body) = session();
    assert_eq!(status, 200);
    assert!(body.contains(r#"{"session":{"webDisabled":true}}"#) && !body.contains("model"), "{body}");
    assert_eq!((shell(), metrics()), (403, 200));
    assert_eq!(http(port, "GET", "/", "").0, 200);
    assert_eq!(http(port, "POST", "/opendeskphone.v1.StatusService/GetStatus", "{}").0, 403);
    assert_eq!(http(port, "POST", "/upload/firmware", "x").0, 403);

    // Metrics off as well: the settings file, then a reload.
    let settings = dir.path().join("nv/opendeskphone/settings.txtpb");
    std::fs::write(&settings, "metrics_enabled: false\n").unwrap();
    assert_eq!(diag(&socket, &["config", "reload"]).0, 0);
    wait_for("metrics to be disabled", Duration::from_secs(10), || metrics() == 404);
    assert_eq!(login(), 403);

    // Web configuration back on; metrics still off.
    set("web_ui_enabled", "1");
    wait_for("web configuration to be enabled", Duration::from_secs(10), || login() == 200);
    assert_eq!((shell(), metrics()), (401, 404));

    // Metrics back on.
    std::fs::write(&settings, "metrics_enabled: true\n").unwrap();
    assert_eq!(diag(&socket, &["config", "reload"]).0, 0);
    wait_for("metrics to be enabled", Duration::from_secs(10), || metrics() == 200);
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-V").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

#[test]
fn ssh_exec_and_sftp_with_openssh_client() {
    if !have("ssh") || !have("scp") || !Path::new("/bin/sh").exists() {
        test_inputs::unavailable("no OpenSSH client (ssh, scp) or no /bin/sh");
        return;
    }
    let dir = temp();
    let d = dir.path();
    let key = d.join("id");
    let bad = d.join("bad");
    for k in [&key, &bad] {
        let ok = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(k).status().unwrap();
        assert!(ok.success());
    }
    // An RSA key too: that is what the stock phones' authorized_keys hold.
    let rsa = d.join("id_rsa");
    let ok = Command::new("ssh-keygen").args(["-q", "-t", "rsa", "-b", "3072", "-N", "", "-f"]).arg(&rsa).status().unwrap();
    assert!(ok.success());
    // The stock firmware's key file, as on a phone that has run both
    // firmwares: the stock firmware's key, and a key added under an earlier
    // Open Desk Phone.  The first start moves the added key only.
    let stock_file = d.join("nv/root/.ssh/authorized_keys");
    std::fs::create_dir_all(stock_file.parent().unwrap()).unwrap();
    let id_pub = std::fs::read_to_string(d.join("id.pub")).unwrap();
    let release = opendeskphone_config::nvdata::STOCK_FIRMWARE_KEYS[0];
    std::fs::write(&stock_file, format!("ssh-dss {release} stock\n{id_pub}")).unwrap();
    std::fs::create_dir_all(d.join("nv/opendeskphone")).unwrap();
    // SSH is off unless the settings turn it on.
    std::fs::write(d.join("nv/opendeskphone/settings.txtpb"), "ssh_enabled: true\n").unwrap();

    let nv = d.join("nv");
    let run = d.join("run");
    let mut child = Command::new(BIN)
        .args(["main", "--sim", "--ssh-listen", "127.0.0.1:0", "--nvdata"])
        .arg(&nv)
        .arg("--run-dir")
        .arg(&run)
        .env("DPHONE_SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let _main = KillOnDrop(child);
    let mut port = None;
    for line in BufReader::new(stderr).lines() {
        let line = line.unwrap();
        if let Some(rest) = line.split("ssh listening on 127.0.0.1:").nth(1) {
            port = rest.trim().parse::<u16>().ok();
            break;
        }
    }
    let port = port.expect("ssh port in log").to_string();

    let known = d.join("known_hosts");
    let options = |cmd: &mut Command, identity: &Path| {
        cmd.args(["-o", "StrictHostKeyChecking=no", "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"])
            .args(["-o", "IdentityAgent=none", "-o", "AddKeysToAgent=no", "-o", "ControlMaster=no"])
            .args(["-o", "ControlPath=none", "-o", "LogLevel=ERROR", "-F", "/dev/null"])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", known.display()))
            .arg("-i")
            .arg(identity)
            .stdin(Stdio::null());
    };
    let ssh = |identity: &Path, user: &str, command: &str| {
        let mut cmd = Command::new("ssh");
        options(&mut cmd, identity);
        cmd.args(["-p", &port]).arg(format!("{user}@127.0.0.1")).arg(command);
        cmd.output().unwrap()
    };

    // Moved: the added key logs in, and the stock firmware's key is left out.
    let keys_file = d.join("nv/opendeskphone/authorized_keys");
    assert_eq!(std::fs::read_to_string(&keys_file).unwrap(), id_pub);
    assert_eq!(ssh(&key, "root", "true").status.code(), Some(0));

    // No key is compiled in, so with no key file nobody gets in, and the
    // stock firmware's file is not read again.
    std::fs::remove_file(&keys_file).unwrap();
    assert_eq!(ssh(&key, "root", "true").status.code(), Some(255));
    std::fs::copy(d.join("id_rsa.pub"), &stock_file).unwrap();
    assert_eq!(ssh(&rsa, "root", "true").status.code(), Some(255));

    // Keys are read at each attempt, from both files: the ones added on the
    // phone, and the provisioned ones.  No restart in between.
    std::fs::copy(d.join("id.pub"), &keys_file).unwrap();
    assert_eq!(ssh(&key, "root", "true").status.code(), Some(0));
    assert_eq!(ssh(&rsa, "root", "true").status.code(), Some(255));
    std::fs::copy(d.join("id_rsa.pub"), d.join("nv/opendeskphone/provisioned_authorized_keys")).unwrap();

    // Command, output on both streams, exit status.
    let out = ssh(&key, "root", "echo out; echo err >&2; exit 7");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "out\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("err"));

    // RSA public-key login.
    let out = ssh(&rsa, "root", "echo rsa");
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(0), "rsa\n"), "{out:?}");

    // Unknown key and unknown user are refused.
    assert_eq!(ssh(&bad, "root", "true").status.code(), Some(255));
    assert_eq!(ssh(&key, "admin", "true").status.code(), Some(255));

    // A terminal session.
    let mut cmd = Command::new("ssh");
    options(&mut cmd, &key);
    let out = cmd.args(["-tt", "-p", &port, "root@127.0.0.1", "tty; exit 3"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stdout).contains("/dev/pts/"), "{:?}", out);

    // Push a file with scp (SFTP underneath) and read it back over exec.
    {
        let payload: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        let local = d.join("payload.bin");
        let remote = d.join("pushed.bin");
        std::fs::write(&local, &payload).unwrap();
        let mut cmd = Command::new("scp");
        options(&mut cmd, &key);
        let out = cmd.args(["-P", &port]).arg(&local).arg(format!("root@127.0.0.1:{}", remote.display())).output().unwrap();
        assert!(out.status.success(), "scp: {:?}", out);
        assert_eq!(std::fs::read(&remote).unwrap(), payload);
        let out = ssh(&key, "root", &format!("cat {}", remote.display()));
        assert_eq!(out.stdout, payload);

        // Into a directory, as in `scp file root@phone:/tmp/`, keeping the
        // mode (-p also sends times, which the server accepts).
        let into = d.join("into");
        std::fs::create_dir(&into).unwrap();
        std::fs::set_permissions(&local, std::os::unix::fs::PermissionsExt::from_mode(0o750)).unwrap();
        let mut cmd = Command::new("scp");
        options(&mut cmd, &key);
        let out = cmd.args(["-p", "-P", &port]).arg(&local).arg(format!("root@127.0.0.1:{}/", into.display())).output().unwrap();
        assert!(out.status.success(), "scp into a directory: {:?}", out);
        let pushed = into.join("payload.bin");
        assert_eq!(std::fs::read(&pushed).unwrap(), payload);
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&pushed).unwrap().permissions()) & 0o777, 0o750);

        // And back.
        let fetched = d.join("fetched.bin");
        let mut cmd = Command::new("scp");
        options(&mut cmd, &key);
        let out = cmd.args(["-P", &port]).arg(format!("root@127.0.0.1:{}", pushed.display())).arg(&fetched).output().unwrap();
        assert!(out.status.success(), "scp from the phone: {:?}", out);
        assert_eq!(std::fs::read(&fetched).unwrap(), payload);

        // A directory tree.
        let mut cmd = Command::new("scp");
        options(&mut cmd, &key);
        let out = cmd.args(["-r", "-P", &port]).arg(&into).arg(format!("root@127.0.0.1:{}", d.join("tree").display())).output().unwrap();
        assert!(out.status.success(), "scp -r: {:?}", out);
        assert_eq!(std::fs::read(d.join("tree/payload.bin")).unwrap(), payload);
    }
}

/// The alarm clock in the real main role: an alarm set through it is kept
/// in nvdata, and one that was ringing when the phone went down rings
/// again when it is up, on the ringer and on screen, where the softkeys
/// snooze and dismiss it.  (When each alarm is due is tested with the
/// engine, `src/alarms.rs`, which has a clock that can be moved.)
#[test]
fn the_alarm_clock_keeps_its_alarms_and_rings_after_a_restart() {
    let dir = temp();
    let (mut main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("diag socket", Duration::from_secs(10), || socket.exists());
    wait_for("the alarm clock", Duration::from_secs(10), || diag(&socket, &["alarms"]).0 == 0);
    let (code, out, err) = diag(&socket, &["alarms", "add", "07:00", "--days", "mon,wed", "--tone", "osmium"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("1 07:00 mon,wed Osmium on\n") && out.contains("next: 1 at "), "{out}");
    let (code, _, err) = diag(&socket, &["alarms", "add", "25:00"]);
    assert_eq!(code, 1, "{err}");
    let (code, _, _) = diag(&socket, &["restart"]);
    assert_eq!(code, 0);
    assert_eq!(main.0.wait().unwrap().code(), Some(0));

    // As a phone that lost power while the alarm rang leaves it.
    use opendeskphone_config::saved;
    let file = dir.path().join("nv").join(saved::ALARMS_FILE);
    let mut kept: saved::Alarms = saved::from_text(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(kept.alarms[0].hour, 7);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    kept.pending = saved::MessageField::some(saved::PendingAlarm { id: 1, until: now, unattended: 0 });
    std::fs::write(&file, saved::to_text("opendeskphone.settings.v1.Alarms", &kept)).unwrap();

    let (mut main, socket) = start_main(dir.path(), &["--no-ssh"]);
    wait_for("the alarm to ring", Duration::from_secs(15), || diag(&socket, &["alarms"]).1.contains("ringing: 1\n"));
    wait_for("the alarm's screen", Duration::from_secs(5), || diag(&socket, &["state", "ui"]).1.contains("\"alarm\""));
    let (_, audio, _) = diag(&socket, &["state", "audio"]);
    assert!(audio.contains("ringing: true"), "{audio}");

    // Snooze, the first softkey.
    let (code, _, err) = diag(&socket, &["key", "sk1"]);
    assert_eq!(code, 0, "{err}");
    wait_for("the snooze", Duration::from_secs(5), || diag(&socket, &["alarms"]).1.contains("snoozed: 1 until "));
    let (_, out, _) = diag(&socket, &["alarms"]);
    assert!(!out.contains("ringing"), "{out}");
    wait_for("the ringer to stop", Duration::from_secs(5), || diag(&socket, &["state", "audio"]).1.contains("ringing: false"));
    assert!(!diag(&socket, &["state", "ui"]).1.contains("\"alarm\""));

    let (code, out, _) = diag(&socket, &["alarms", "dismiss"]);
    assert_eq!(code, 0);
    assert!(!out.contains("snoozed") && out.contains("1 07:00 mon,wed Osmium on\n"), "{out}");
    let (code, out, _) = diag(&socket, &["alarms", "remove", "1"]);
    assert_eq!(code, 0);
    assert!(!out.contains("07:00"), "{out}");

    let (code, _, _) = diag(&socket, &["restart"]);
    assert_eq!(code, 0);
    assert_eq!(main.0.wait().unwrap().code(), Some(0));
    let kept: saved::Alarms = saved::from_text(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(kept.alarms.is_empty(), "{kept:?}");
}
