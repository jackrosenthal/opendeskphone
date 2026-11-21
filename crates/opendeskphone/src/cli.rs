//! The command line of the multi-call binary, defined with clap.
//!
//! | Invocation | Role |
//! | --- | --- |
//! | PID 1 (as `/sbin/init`) | init role |
//! | `dphone main [options]` | main role |
//! | `dphone mount ...`, or a symlink named `mount`, etc. | system command |
//! | `dphone update ...`, `dphone ramboot ...`, `dphone supervise ...` | run in the calling process |
//! | `dphone <anything else>` | diagnostic command sent to the main role |
//!
//! There is one command tree, [`Invocation`].  The commands that run in
//! the calling process ([`Command`]'s own variants and [`SystemCommand`])
//! are defined beside the code that runs them.  So are the commands the
//! running main role answers, each a type its subsystem registers a
//! handler for (see [`crate::diag::Registry`]); [`Remote`] lists them, which
//! is what puts them in the tree.
//!
//! The process that is called parses the whole command line, so `dphone
//! help`, `dphone COMMAND --help`, and the report of a mistake are clap's,
//! made from that one tree, and need no main role.  For a command of
//! [`Remote`] it then sends the arguments, as they were given, to the main
//! role, which parses them again with the same definition into the type
//! the handler takes.  (Sending the arguments, not the parsed value, keeps
//! the socket's protocol a list of words: there is nothing to serialize,
//! and whatever else speaks it, e.g., a test, is checked the same way.
//! Both ends are this one binary, so the two parses cannot disagree.)

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

use crate::cmds::SystemCommand;
use crate::diag::builtin::{HealthCommand, LogCommand, PingCommand, RestartCommand, VersionCommand};
use crate::init;
use crate::mainrole::subsystems::{
    ConfigCommand, StateCommand,
    alarms::AlarmsCommand,
    audio::{AudioCommand, DspCommand},
    ehs::EhsCommand,
    expansion::ExpansionCommand,
    hal::HalCommand,
    liveness::CrashlogCommand,
    net::NetCommand,
    paging::PageCommand,
    remote::{AppCommand, CallCommand, HookCommand, KeyCommand, ScreenshotCommand, TouchCommand},
};

/// What the binary was called as: `dphone`, or the name of one of the
/// system commands it provides (the image has a symbolic link for each).
#[derive(Debug, Parser)]
#[command(multicall = true)]
enum Invocation {
    /// Open Desk Phone: the phone's one program
    #[command(version = crate::VERSION, after_help = where_commands_run())]
    Dphone {
        /// The main role's diagnostics socket, for the commands it answers
        /// [default: /run/opendeskphone/diag.sock]
        #[arg(long, value_name = "PATH", env = "DPHONE_DIAG_SOCKET")]
        diag_socket: Option<std::path::PathBuf>,
        #[command(subcommand)]
        command: Command,
    },
    #[command(flatten)]
    System(SystemCommand),
    /// Any other name (e.g., `/sbin/init` when it is not PID 1, or a copy
    /// under test): `dphone`.
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the main role
    Main(crate::mainrole::Options),
    /// Run the init role's supervisor without the duties of PID 1 (for tests)
    ///
    /// The init role proper runs only as PID 1.
    Supervise(Supervise),
    /// Firmware: show the banks, install an image, select a bank
    Update {
        #[command(subcommand)]
        command: crate::update_cli::UpdateCommand,
    },
    /// Start an image from RAM without installing it
    Ramboot(crate::ramboot_cli::Ramboot),
    /// Print the shell's tab completion of `dphone`
    ///
    /// A Bash script, which brush reads, made from this command tree.  The
    /// phone's `/etc/bash.bashrc` loads it the first time a `dphone`
    /// command line is completed.
    Completion,
    #[command(flatten)]
    System(SystemCommand),
    #[command(flatten)]
    Remote(Remote),
}

/// The commands the running main role answers.  A subsystem adds its
/// command here and registers the handler when it starts.
#[derive(Debug, Subcommand)]
pub enum Remote {
    Alarms(AlarmsCommand),
    App(AppCommand),
    Audio(AudioCommand),
    Call(CallCommand),
    Config(ConfigCommand),
    Crashlog(CrashlogCommand),
    Dsp(DspCommand),
    Ehs(EhsCommand),
    Expansion(ExpansionCommand),
    Hal(HalCommand),
    Health(HealthCommand),
    Hook(HookCommand),
    Key(KeyCommand),
    Log(LogCommand),
    Net(NetCommand),
    Page(PageCommand),
    Ping(PingCommand),
    Restart(RestartCommand),
    Screenshot(ScreenshotCommand),
    State(StateCommand),
    Touch(TouchCommand),
    Version(VersionCommand),
}

/// Which commands need the main role, for the end of `dphone --help`.
fn where_commands_run() -> String {
    let names = |cmd: clap::Command| cmd.get_subcommands().map(|c| c.get_name().to_string()).collect::<Vec<_>>();
    let remote = names(Remote::augment_subcommands(clap::Command::new("dphone")));
    let mut local = names(Command::augment_subcommands(clap::Command::new("dphone")));
    local.retain(|name| !remote.contains(name));
    format!(
        "These run in the calling process, with or without a main role:\n  {}\n\
         The other commands are answered by the running main role.",
        local.join(" ")
    )
}

/// `on` or `off`, for the commands that switch something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    pub fn is_on(self) -> bool {
        self == OnOff::On
    }
}

#[derive(Debug, Args)]
struct Supervise {
    /// The nvdata directory
    #[arg(long, value_name = "DIR")]
    nvdata: PathBuf,
    /// The runtime directory
    #[arg(long, value_name = "DIR")]
    run_dir: PathBuf,
    /// How long the program has to report itself healthy after a start
    #[arg(long, value_name = "SECONDS", default_value_t = init::START_GRACE_SECS)]
    grace: u64,
    /// How long the program may go without a heartbeat
    #[arg(long, value_name = "SECONDS", default_value_t = init::BEAT_TIMEOUT_SECS)]
    beat_timeout: u64,
    /// How long the program may stay unhealthy before the phone would reboot
    #[arg(long, value_name = "SECONDS", default_value_t = init::UNHEALTHY_REBOOT_SECS)]
    unhealthy_reboot: u64,
    /// A file to write in place of a return to the other bank
    #[arg(long, value_name = "FILE")]
    rollback_marker: Option<PathBuf>,
    /// The program to supervise as the main role, and its arguments
    #[arg(last = true, required = true, value_name = "PROGRAM")]
    program: Vec<String>,
}

/// Hooks other crates supply to the init role.  The updater's rollback
/// function is added here when that crate is wired in.
fn init_hooks() -> init::Hooks {
    init::Hooks { rollback: Some(crate::update_cli::rollback_hook) }
}

/// The whole command tree, e.g., for a front end that shows help.
pub fn command() -> clap::Command {
    Invocation::command()
}

/// `dphone completion`: the Bash completion of `dphone` and every command
/// under it, by clap_complete.  The root of the tree is the multicall
/// root, which nobody runs, so the script is made from `dphone`'s part.
fn completion_script() -> Vec<u8> {
    let mut dphone = command().find_subcommand("dphone").expect("'dphone' is a name of the tree").clone();
    let mut script = Vec::new();
    clap_complete::generate(clap_complete::shells::Bash, &mut dphone, "dphone", &mut script);
    script
}

pub fn run(args: Vec<OsString>) -> i32 {
    // PID 1 is always the init role, whatever the name or arguments.
    if crate::sys::getpid() == 1 {
        init::run(init_hooks());
    }
    let invocation = match Invocation::try_parse_from(&args) {
        Ok(Invocation::Other(mut words)) => {
            words[0] = "dphone".into();
            Invocation::try_parse_from(&words)
        }
        other => other,
    };
    match invocation {
        Ok(Invocation::Dphone { diag_socket, command }) => match command {
            Command::Main(options) => crate::mainrole::run(options),
            Command::Supervise(s) => init::run_supervisor_test(init::TestOptions {
                paths: crate::paths::Paths::new(s.nvdata, s.run_dir),
                main_command: s.program,
                start_grace: s.grace,
                beat_timeout: s.beat_timeout,
                unhealthy_reboot: s.unhealthy_reboot,
                rollback_marker: s.rollback_marker,
            }),
            Command::Update { command } => crate::update_cli::run(command),
            Command::Ramboot(ramboot) => crate::ramboot_cli::run(&ramboot),
            Command::Completion => {
                crate::out::stdout_bytes(&completion_script());
                0
            }
            Command::System(command) => crate::cmds::run(command),
            // Checked here; parsed again, into the handler's type, there.
            Command::Remote(_) => {
                let words: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
                let socket = diag_socket.unwrap_or_else(crate::paths::default_diag_socket);
                crate::diag::client::run(&socket, &without_diag_socket(words))
            }
        },
        Ok(Invocation::System(command)) => crate::cmds::run(command),
        Ok(Invocation::Other(_)) => unreachable!("'dphone' is a name of the tree"),
        Err(e) => report(&e),
    }
}

/// A command line after `dphone`, without `--diag-socket` (which comes
/// before the command): the words the main role parses.
fn without_diag_socket(words: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    let mut words = words.into_iter();
    while let Some(w) = words.next() {
        if w == "--diag-socket" {
            words.next();
        } else if !w.starts_with("--diag-socket=") {
            out.push(w);
            out.extend(words);
            break;
        }
    }
    out
}

/// Print help that was asked for, or what is wrong with the command line;
/// returns the exit status (0 and 2, as clap has them).
fn report(e: &clap::Error) -> i32 {
    let text = e.render().to_string();
    if e.use_stderr() {
        crate::err!("{text}");
        2
    } else {
        crate::out!("{text}");
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Invocation, clap::Error> {
        Invocation::try_parse_from(words)
    }

    /// Every command of the tree, with the words that reach it.
    fn every_command(cmd: &clap::Command, path: &mut Vec<String>, found: &mut Vec<Vec<String>>) {
        found.push(path.clone());
        for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
            path.push(sub.get_name().to_string());
            every_command(sub, path, found);
            path.pop();
        }
    }

    fn all_paths() -> Vec<Vec<String>> {
        let mut found = Vec::new();
        every_command(&command(), &mut Vec::new(), &mut found);
        // The multicall root itself is not a command anyone can run.
        found.remove(0);
        found
    }

    #[test]
    fn the_diag_socket_is_not_forwarded() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(without_diag_socket(words(&["--diag-socket", "/s", "net", "status"])), words(&["net", "status"]));
        assert_eq!(without_diag_socket(words(&["--diag-socket=/s", "ping"])), words(&["ping"]));
        // After the command, the word is the command's.
        assert_eq!(without_diag_socket(words(&["say", "--diag-socket"])), words(&["say", "--diag-socket"]));
        assert!(matches!(
            parse(&["dphone", "--diag-socket", "/s", "ping"]),
            Ok(Invocation::Dphone { diag_socket: Some(_), command: Command::Remote(_) })
        ));
    }

    #[test]
    fn every_command_has_a_description() {
        let paths = all_paths();
        // Local commands, system commands by name and under `dphone`, the
        // main role's commands, and their subcommands.
        assert!(paths.len() > 80, "{}", paths.len());
        for path in paths {
            let mut cmd = command();
            let mut at = &mut cmd;
            for word in &path {
                at = at.find_subcommand_mut(word).unwrap();
            }
            assert!(at.get_about().is_some(), "{path:?} has no description");
        }
    }

    /// Each of the main role's commands is in the tree under the name its
    /// own definition has, which is the name its handler is registered and
    /// looked up by.
    #[test]
    fn the_main_roles_commands_are_in_the_tree_under_their_own_names() {
        let tree = Remote::augment_subcommands(clap::Command::new("dphone"));
        let in_tree: Vec<&str> = tree.get_subcommands().map(|c| c.get_name()).collect();
        let own = [
            AlarmsCommand::command(),
            AppCommand::command(),
            AudioCommand::command(),
            CallCommand::command(),
            ConfigCommand::command(),
            CrashlogCommand::command(),
            DspCommand::command(),
            EhsCommand::command(),
            ExpansionCommand::command(),
            HalCommand::command(),
            HealthCommand::command(),
            HookCommand::command(),
            KeyCommand::command(),
            LogCommand::command(),
            NetCommand::command(),
            PageCommand::command(),
            PingCommand::command(),
            RestartCommand::command(),
            ScreenshotCommand::command(),
            StateCommand::command(),
            TouchCommand::command(),
            VersionCommand::command(),
        ];
        let own_names: Vec<&str> = own.iter().map(|c| c.get_name()).collect();
        assert_eq!(in_tree, own_names);
    }

    /// The keys and lamps of the main role's commands are named as the
    /// phone names them, and a touch is a place.
    #[test]
    fn keys_and_lamps_by_name() {
        let ok = |words: &[&str]| {
            let mut all = vec!["dphone"];
            all.extend(words);
            assert!(matches!(parse(&all), Ok(Invocation::Dphone { command: Command::Remote(_), .. })), "{words:?}");
        };
        let bad = |words: &[&str]| {
            let mut all = vec!["dphone"];
            all.extend(words);
            assert!(parse(&all).is_err(), "{words:?}");
        };
        ok(&["key", "1", "2", "3", "sk1", "#", "*"]);
        ok(&["key", "line6", "vup"]);
        bad(&["key", "1", "nope"]);
        ok(&["touch", "400", "640"]);
        ok(&["touch", "400", "1000", "--to", "400", "300", "--steps", "4"]);
        bad(&["touch", "400"]);
        bad(&["touch", "-1", "5"]);
        bad(&["touch", "1", "2", "--to", "3"]);
        bad(&["touch", "1", "2", "--to", "3", "4", "--steps", "0"]);
        ok(&["hal", "lamp", "line3", "green"]);
        ok(&["hal", "lamp", "all", "off"]);
        bad(&["hal", "lamp", "line0", "green"]);
    }
}
