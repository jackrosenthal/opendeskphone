//! `dphone update ...`: firmware update commands that run in the calling
//! process, not through the diag socket, so they work over SSH or the serial
//! console even when the main role is down.
//!
//! This is also the way back to stock firmware: `dphone update install
//! FILE.eff` accepts a stock EFF.
//!
//! On a D80 nothing is installed by the running system: `install` stores
//! the image's package in nvdata, writes the recovery that the package
//! brings, and has the boot loader start that recovery at the next
//! restart, which installs the package (`opendeskphone_update::d80`).
//!
//! An image need not be copied to the phone first (its `/tmp` is RAM, and
//! smaller than two images): `dphone update install -` reads it from
//! standard input (`ssh root@PHONE dphone update install - < FILE.eff`),
//! and `dphone update install http://...` downloads it, in both cases
//! straight into the bank that is not running.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use opendeskphone_hal::Mtd;
use opendeskphone_hal::phone::PhoneMtd;
use opendeskphone_update::{Bank, Options, Updater, inspect, select_bank, trial};

/// Where `install` reads the image from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Stdin,
    Url(String),
}

fn parse_source(s: &str) -> Result<Source, String> {
    Ok(if s == "-" {
        Source::Stdin
    } else if s.starts_with("http://") || s.starts_with("https://") {
        Source::Url(s.to_string())
    } else {
        Source::File(PathBuf::from(s))
    })
}

use crate::init::trial::TrialRecord;

#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Show both banks, the ctrl record, and what boot2 will boot next
    ///
    /// On a D80, which has no banks: what the next restart starts, the
    /// package left for the recovery, and what the recovery did with the
    /// last one.
    Status,
    /// Install an image into the bank that is not running, and select it
    ///
    /// The image (Open Desk Phone or stock) is written, read back and compared,
    /// and selected for the next boot; reboot afterwards with 'dphone
    /// reboot'.  A file is read and checked in full before anything is
    /// written.  An image from standard input or a URL goes straight into
    /// the bank that is not running, without being stored on the phone;
    /// should it turn out bad, that bank's old contents are gone and
    /// nothing else has changed.  One update runs at a time, and none while
    /// an installed image is not confirmed yet (see 'abandon').
    ///
    /// A D80 has no banks, and its recovery installs an image: the image's
    /// package is stored in nvdata, the recovery that the package brings is
    /// written to the recovery partition, and the next restart starts that
    /// recovery, which installs the package and restarts into it.
    Install {
        /// The image: an EFF file, '-' for standard input, or an http://
        /// or https:// URL (the server's certificate is not checked; the
        /// image's own checksums are)
        #[arg(value_name = "FILE|-|URL", value_parser = parse_source)]
        source: Source,
        /// The bank to write [default: the one that is not running]
        #[arg(long, value_parser = parse_bank)]
        bank: Option<Bank>,
        /// Go ahead even if the bank that stays is not bootable (for
        /// recovery work from a system started from RAM)
        #[arg(long)]
        allow_unbootable_other: bool,
    },
    /// Select a bank for the next boot without writing an image
    ///
    /// Refused unless that bank holds a bootable image.  Removes a trial
    /// record.
    SwitchBank {
        #[arg(value_parser = parse_bank)]
        bank: Bank,
    },
    /// Give up an installed image that is not confirmed yet
    ///
    /// Selects the bank the phone would return to and removes the trial
    /// record, so that another image can be installed.  When the phone is
    /// already running the image given up, reboot afterwards.
    ///
    /// On a D80: takes back a package left for the recovery, before the
    /// restart that would install it.
    Abandon,
    /// Record a trial boot of the bank 'ctrl' selects for the next boot
    ///
    /// For an image installed and selected by other means (e.g., by the
    /// stock updater, or with switch-bank, which must come first because it
    /// removes a trial record): Open Desk Phone returns to the other bank by
    /// itself if the image does not come up healthy within a few boots.
    ArmTrial {
        /// The bank to return to [default: the running one]
        ///
        /// The running bank is the one in root= on the kernel command line;
        /// when that is the selected bank itself or neither bank (a kernel
        /// started from RAM), name the bank to return to.
        #[arg(long, value_name = "BANK", value_parser = parse_bank)]
        return_to: Option<Bank>,
    },
    /// Erase the unused 'userdata' partition and write a root file system image there
    ///
    /// For booting a test kernel from RAM with root=/dev/mtdblockN without
    /// touching either firmware bank.
    WriteUserdata {
        /// The image (JFFS2); '-' reads it from standard input
        image: PathBuf,
    },
}

fn cmdline() -> String {
    std::fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

fn model() -> Option<opendeskphone_state::Model> {
    let detected = crate::model::detect()?;
    detected.model.name().parse().ok()
}

fn open_mtd() -> Result<PhoneMtd, String> {
    PhoneMtd::open().map_err(|e| format!("flash not available: {e}"))
}

fn parse_bank(s: &str) -> Result<Bank, String> {
    match s {
        "a" | "A" => Ok(Bank::A),
        "b" | "B" => Ok(Bank::B),
        _ => Err("not a bank (a or b)".into()),
    }
}

fn bank_name(b: Bank) -> &'static str {
    match b {
        Bank::A => "A (k0/root0)",
        Bank::B => "B (k1/root1)",
    }
}

fn own_dir() -> std::path::PathBuf {
    crate::paths::Paths::default().own_dir()
}

/// The init role's rollback hook: select the previous bank in `ctrl`.
pub fn rollback_hook(record: &TrialRecord) -> std::io::Result<()> {
    let model = model().ok_or_else(|| std::io::Error::other("model unknown"))?;
    let previous = if record.previous_bank == 0 { Bank::A } else { Bank::B };
    let mut mtd = PhoneMtd::open().map_err(|e| std::io::Error::other(e.to_string()))?;
    trial::rollback(&mut mtd, model, &cmdline(), previous).map_err(Into::into)
}

pub fn run(command: UpdateCommand) -> i32 {
    let result = match command {
        UpdateCommand::Status => status(),
        UpdateCommand::Install { source, bank, allow_unbootable_other } => {
            install(&source, Options { target: bank, allow_unbootable_other, ..Options::default() })
        }
        UpdateCommand::Abandon => abandon(),
        UpdateCommand::SwitchBank { bank } => switch_bank(bank),
        UpdateCommand::ArmTrial { return_to } => arm_trial(return_to),
        UpdateCommand::WriteUserdata { image } => write_userdata(&image),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            crate::errln!("dphone update: {e}");
            1
        }
    }
}

fn status() -> Result<(), String> {
    if model().is_some_and(|m| m.is_touch()) {
        return status_d80();
    }
    let mut mtd = open_mtd()?;
    let dir = own_dir();
    let st = inspect(&mut mtd, &cmdline(), dir.exists().then_some(dir.as_path())).map_err(|e| e.to_string())?;
    let name = |b: Option<Bank>| b.map(bank_name).unwrap_or("neither bank");
    crate::outln!("running from:   {}", name(st.running));
    crate::outln!("ctrl active:    {}", name(st.active));
    crate::outln!("boot2 boots:    {}", bank_name(st.next_boot));
    crate::outln!(
        "ctrl record:    {}, {} valid copies ({} good blocks, {} bad){}",
        if st.ctrl_valid { "valid" } else { "INVALID" },
        st.ctrl_copies.0,
        st.ctrl_copies.1,
        st.ctrl_copies.2,
        if st.ctrl_consistent { "" } else { ", copies DISAGREE" },
    );
    for b in &st.banks {
        crate::outln!("bank {}:", bank_name(b.bank));
        crate::outln!("  header:       {}", if b.header.is_some() { "valid" } else { "none or bad CRC" });
        crate::outln!("  image:        {} ({})", b.image_name, b.build_date);
        crate::outln!("  version:      {}", b.version.as_deref().unwrap_or("?"));
        match &b.kernel {
            Ok(span) => crate::outln!("  kernel:       loadable ({span:?})"),
            Err(p) => crate::outln!("  kernel:       NOT loadable: {p}"),
        }
        crate::outln!("  bad blocks:   kernel {}, rootfs {}", b.kernel_bad_blocks, b.rootfs_bad_blocks);
        crate::outln!("  boot2 boots:  {}", if b.bootable() { "yes (valid header, loadable kernel)" } else { "no" });
        // boot2 checks no more than that.  Whether the bank holds the
        // image its header describes takes reading all of it back.
        match &b.header {
            Some(h) => match opendeskphone_update::verify_bank(&mut mtd, b.bank, h).map_err(|e| e.to_string())? {
                Ok(()) => crate::outln!(
                    "  contents:     kernel and root file system match the header's lengths and checksums"
                ),
                Err(why) => crate::outln!("  contents:     DAMAGED: {why}"),
            },
            None => crate::outln!("  contents:     not checked (no header to check against)"),
        }
    }
    match &st.trial {
        Some(t) => crate::outln!("trial:          pending ({t:?})"),
        None => crate::outln!("trial:          none"),
    }
    Ok(())
}

/// A D80 has no banks to report: one kernel and one root file system, on
/// the eMMC, and a recovery that installs a package left for it.
fn status_d80() -> Result<(), String> {
    let line = cmdline();
    let root = line.split_ascii_whitespace().find_map(|w| w.strip_prefix("root=")).unwrap_or("?");
    let installed = if root == crate::init::nvdata::D80_ROOT { "the installed firmware" } else { "not the installed firmware" };
    crate::outln!("running from:   {root} ({installed})");
    #[cfg(feature = "d80")]
    {
        let st = opendeskphone_update::d80::status(&d80_places());
        match &st.bootcmd {
            Ok(command) => crate::outln!(
                "next restart:   {} (bootcmd \"{command}\")",
                if st.recovery_next { "the recovery" } else { "the system" }
            ),
            Err(e) => crate::outln!("next restart:   the system, by the boot loader's own settings ({e})"),
        }
        match (&st.command, st.package_len) {
            (Some(package), Some(len)) => crate::outln!("left to install: {package} ({len} bytes)"),
            (Some(package), None) => crate::outln!("left to install: {package}, which is not in nvdata"),
            (None, Some(len)) => crate::outln!("left to install: nothing (a package of {len} bytes is in nvdata, with no command)"),
            (None, None) => crate::outln!("left to install: nothing"),
        }
        match &st.last_install {
            Some((package, true)) => crate::outln!("last install:   {package}, installed"),
            Some((package, false)) => crate::outln!("last install:   {package}, NOT installed (the recovery's log: /nvdata/recovery/last_log)"),
            None => crate::outln!("last install:   none recorded"),
        }
    }
    #[cfg(not(feature = "d80"))]
    crate::outln!("{}", opendeskphone_update::D80_NOT_HERE);
    Ok(())
}

#[cfg(feature = "d80")]
fn d80_places() -> opendeskphone_update::d80::Places {
    opendeskphone_update::d80::Places::phone(&crate::paths::Paths::default().nvdata)
}

/// `install` on a D80: the package is left for the recovery.
#[cfg(feature = "d80")]
fn install_d80(source: &Source) -> Result<(), String> {
    use opendeskphone_update::d80;
    let mut last = String::new();
    let progress = |phase: &opendeskphone_state::UpdatePhase| {
        // The length of a stream is not known: no share of it is shown.
        let text = match phase {
            opendeskphone_state::UpdatePhase::Receiving { .. } => "Receiving".to_string(),
            other => format!("{other:?}"),
        };
        if text != last {
            crate::out!("\r{text:<40}");
            crate::out::flush();
            last = text;
        }
    };
    let (places, own) = (d80_places(), own_dir());
    let left = match source {
        Source::File(path) => {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            d80::leave_for_recovery(std::io::BufReader::with_capacity(64 * 1024, file), &places, &own, progress)
        }
        Source::Stdin => d80::leave_for_recovery(std::io::stdin().lock(), &places, &own, progress),
        Source::Url(url) => {
            use opendeskphone_dpma::firmware::FirmwareSource;
            let stream = opendeskphone_dpma::fetch::HttpSource::new()
                .open(url, &format!("opendeskphone-update/{}", env!("CARGO_PKG_VERSION")))
                .map_err(|e| format!("{url}: {}", e.message))?;
            d80::leave_for_recovery(stream, &places, &own, progress)
        }
    }
    .map_err(|e| format!("\n{e}"))?;
    crate::outln!();
    crate::outln!(
        "version {}{} is left for the recovery to install ({} bytes)",
        left.version,
        if left.is_stock { " (stock firmware)" } else { "" },
        left.package_len
    );
    crate::outln!(
        "the recovery partition {}",
        match (left.recovery_written, left.is_stock) {
            (true, true) => "was written with the stock recovery, made from the package",
            (true, false) => "was written with the package's recovery",
            (false, _) => "has the package's recovery already",
        }
    );
    crate::outln!("'dphone reboot' to install it; 'dphone update abandon' to take it back");
    Ok(())
}

fn switch_bank(bank: Bank) -> Result<(), String> {
    let model = model().ok_or("model unknown (no digium_board= and no devicetree match)")?;
    let mut mtd = open_mtd()?;
    let _lock = opendeskphone_update::lock::acquire(&own_dir()).map_err(|e| e.to_string())?;
    select_bank(&mut mtd, model, bank).map_err(|e| e.to_string())?;
    // A manual choice ends any trial: the init role must not undo it.
    let _ = trial::clear(&own_dir());
    crate::outln!("bank {} selected for the next boot; 'dphone reboot' to boot it", bank_name(bank));
    Ok(())
}

fn arm_trial(return_to: Option<Bank>) -> Result<(), String> {
    let mut mtd = open_mtd()?;
    let record = trial::arm(&mut mtd, &cmdline(), &own_dir(), return_to).map_err(|e| e.to_string())?;
    let on_trial = record.trial_bank.unwrap_or(record.previous_bank.other());
    crate::outln!(
        "trial recorded: boot2 boots bank {} next; it must come up healthy within {} boots, or the phone returns to bank {}",
        bank_name(on_trial),
        record.max_boots,
        bank_name(record.previous_bank)
    );
    Ok(())
}

fn abandon() -> Result<(), String> {
    let model = model().ok_or("model unknown (no digium_board= and no devicetree match)")?;
    #[cfg(feature = "d80")]
    if model.is_touch() {
        if opendeskphone_update::d80::take_back(&d80_places(), &own_dir()).map_err(|e| e.to_string())? {
            crate::outln!("the package left for the recovery is taken back; the next restart is the system's");
        } else {
            crate::outln!("no package is left for the recovery; nothing to take back");
        }
        return Ok(());
    }
    let mut mtd = open_mtd()?;
    match opendeskphone_update::abandon(&mut mtd, model, &own_dir()).map_err(|e| e.to_string())? {
        Some(bank) => {
            crate::outln!("the unconfirmed image is given up; bank {} is selected for the next boot", bank_name(bank));
            let running = mtd.partitions().ok().and_then(|p| opendeskphone_update::bank::running_bank(&cmdline(), &p));
            if running != Some(bank) {
                crate::outln!("this system is not running from that bank: 'dphone reboot' to return to it");
            }
        }
        None => crate::outln!("no installed image is waiting to be confirmed; nothing to give up"),
    }
    Ok(())
}

fn install(source: &Source, options: Options) -> Result<(), String> {
    let model = model().ok_or("model unknown (no digium_board= and no devicetree match)")?;
    #[cfg(feature = "d80")]
    if model.is_touch() {
        if options.target.is_some() || options.allow_unbootable_other {
            return Err("a D80 has no firmware banks: --bank and --allow-unbootable-other do not apply".into());
        }
        return install_d80(source);
    }
    let mut mtd = open_mtd()?;
    let parts = mtd.partitions().map_err(|e| e.to_string())?;
    if parts.is_empty() {
        return Err("no MTD partitions".into());
    }
    let mut last = String::new();
    let updater = Updater::new(&mut mtd, model, &cmdline(), &own_dir()).options(options).progress(|phase| {
        let text = format!("{phase:?}");
        if text != last {
            crate::out!("\r{text:<40}");
            crate::out::flush();
            last = text;
        }
    });
    let mut updater = updater;
    let installed = match source {
        // A file can be read twice: all of it is checked before the first
        // erase.
        Source::File(path) => {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            updater.install_eff_file(std::io::BufReader::with_capacity(64 * 1024, file))
        }
        Source::Stdin => updater.install_eff(std::io::stdin().lock()),
        Source::Url(url) => {
            use opendeskphone_dpma::firmware::FirmwareSource;
            let stream = opendeskphone_dpma::fetch::HttpSource::new()
                .open(url, &format!("opendeskphone-update/{}", env!("CARGO_PKG_VERSION")))
                .map_err(|e| format!("{url}: {}", e.message))?;
            updater.install_eff(stream)
        }
    }
    .map_err(|e| format!("\n{e}"))?;
    crate::outln!();
    crate::outln!(
        "installed version {} into bank {}{}; bank {} is kept as it was",
        installed.version,
        bank_name(installed.bank),
        if installed.is_stock { " (stock firmware)" } else { "" },
        bank_name(installed.previous),
    );
    if !installed.is_stock {
        crate::outln!(
            "trial boot recorded: the phone returns to the kept bank if the new image does not come up healthy"
        );
    }
    crate::outln!("'dphone reboot' to boot it");
    Ok(())
}

/// Write a root file system image to `userdata`, which stock leaves unused.
/// Neither bank and nothing boot2 reads is touched.
fn write_userdata(file: &Path) -> Result<(), String> {
    const PART: &str = "userdata";
    // Streamed one erase block at a time ("-" is standard input): a stock
    // phone has too little free memory to hold the image twice.
    let mut input: Box<dyn Read> = if file == Path::new("-") {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(File::open(file).map_err(|e| format!("{}: {e}", file.display()))?)
    };
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut mtd = open_mtd()?;
    let parts = mtd.partitions().map_err(|e| e.to_string())?;
    let info = parts.iter().find(|p| p.name == PART).ok_or("no partition named userdata")?.clone();
    if mounts.lines().any(|l| l.split_ascii_whitespace().next() == Some(&format!("/dev/mtdblock{}", info.index))) {
        return Err("userdata is mounted; refusing to overwrite it".into());
    }
    let mut part = mtd.open(PART).map_err(|e| e.to_string())?;
    crate::outln!("erasing {PART} ({} bytes)", info.size);
    let bad = opendeskphone_hal::erase_all(part.as_mut(), |_, _| {}).map_err(|e| e.to_string())?;
    crate::outln!("writing ({bad} bad blocks skipped)");
    let bs = info.erase_size as usize;
    let mut block = vec![0u8; bs];
    let mut back = vec![0u8; bs];
    let mut off = 0u64;
    let mut total = 0usize;
    loop {
        let mut n = 0;
        while n < bs {
            match input.read(&mut block[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("{}: {e}", file.display())),
            }
        }
        if n == 0 {
            break;
        }
        let start = off;
        off = opendeskphone_hal::write_skip_bad(part.as_mut(), start, &block[..n], |_, _| {}).map_err(|e| e.to_string())?;
        opendeskphone_hal::read_skip_bad(part.as_mut(), start, &mut back[..n]).map_err(|e| e.to_string())?;
        if back[..n] != block[..n] {
            return Err(format!("read-back differs from the image in the block at {start:#x}"));
        }
        total += n;
        if n < bs {
            break;
        }
    }
    crate::outln!("wrote and verified {total} bytes; boot with root=/dev/mtdblock{} rootfstype=jffs2 ro", info.index);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(subcommand)]
        command: UpdateCommand,
    }

    fn parse(words: &[&str]) -> Result<UpdateCommand, clap::Error> {
        Cli::try_parse_from(std::iter::once(&"update").chain(words)).map(|c| c.command)
    }

    #[test]
    fn commands() {
        assert!(matches!(parse(&["status"]), Ok(UpdateCommand::Status)));
        let Ok(UpdateCommand::Install { source, bank, allow_unbootable_other }) = parse(&["install", "/tmp/fw.eff"]) else {
            panic!("install does not parse")
        };
        assert_eq!((source, bank, allow_unbootable_other), (Source::File("/tmp/fw.eff".into()), None, false));
        for (word, want) in [
            ("-", Source::Stdin),
            ("http://10.0.0.1/fw.eff", Source::Url("http://10.0.0.1/fw.eff".into())),
            ("https://example.org/fw.eff", Source::Url("https://example.org/fw.eff".into())),
            ("./-", Source::File("./-".into())),
        ] {
            let Ok(UpdateCommand::Install { source, .. }) = parse(&["install", word]) else { panic!("{word}") };
            assert_eq!(source, want);
        }
        assert!(matches!(parse(&["abandon"]), Ok(UpdateCommand::Abandon)));
        let Ok(UpdateCommand::Install { bank, allow_unbootable_other, .. }) =
            parse(&["install", "--bank", "b", "fw.eff", "--allow-unbootable-other"])
        else {
            panic!("install with options does not parse")
        };
        assert_eq!((bank, allow_unbootable_other), (Some(Bank::B), true));
        assert!(matches!(parse(&["switch-bank", "A"]), Ok(UpdateCommand::SwitchBank { bank: Bank::A })));
        assert!(matches!(parse(&["arm-trial"]), Ok(UpdateCommand::ArmTrial { return_to: None })));
        assert!(matches!(parse(&["arm-trial", "--return-to", "a"]), Ok(UpdateCommand::ArmTrial { return_to: Some(Bank::A) })));
        assert!(matches!(parse(&["write-userdata", "-"]), Ok(UpdateCommand::WriteUserdata { .. })));
    }

    #[test]
    fn mistakes() {
        for bad in [
            &["install", "a.eff", "--bank", "c"][..],
            &["switch-bank", "2"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
