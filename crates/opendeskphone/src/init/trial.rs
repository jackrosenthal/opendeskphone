//! Trial-boot bookkeeping (section 12).
//!
//! boot2 never falls back by itself.  After an update, the updater records a
//! pending trial here.  Every boot of the trial counts; when the main role
//! reports healthy the trial is confirmed and the record removed; when the
//! count runs out, or the main role keeps failing, the phone flips `ctrl`
//! back to the previous bank and reboots.
//!
//! The counting, and going back when the count runs out or the root file
//! system is unusable, is done by the early-boot stage in the kernel
//! partition (`opendeskphone-early`), before this program is needed; it says so
//! with `DPHONE_EARLY=1` in the environment.  The init role then only
//! confirms the trial, or goes back when the main role keeps failing.  It
//! still counts by itself when started by a kernel without that stage.
//!
//! The record is a small text file, `/nvdata/opendeskphone/trial`:
//!
//! ```text
//! pending=1
//! previous_bank=0
//! boots=1
//! max_boots=3
//! ```

use std::io;
use std::path::{Path, PathBuf};

/// Boots a trial gets before rollback, unless the record says otherwise.
pub const DEFAULT_MAX_BOOTS: u32 = 3;

/// How long a trial boot may take to report healthy before the init role
/// restarts the phone (the boot stays counted against the trial).
pub const TRIAL_HEALTHY_SECS: u64 = 300;

/// Main-role failures within one trial boot that trigger rollback at once.
pub const MAX_TRIAL_CRASHES: u32 = 5;

/// Whether a record is about the boot of bank `running` (0 or 1): not when
/// the system was started from RAM, and not when the bank running is the
/// one to return to (the phone went back, or has not restarted into the
/// new image yet).  The same rule as the early stage's
/// (`opendeskphone_ctrl::boot::step`).
pub fn is_about(record: &TrialRecord, running: Option<u8>) -> bool {
    running.is_some_and(|bank| bank != record.previous_bank)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialRecord {
    /// The bank (0 or 1) that was active before the update; rollback target.
    pub previous_bank: u8,
    /// Boots started so far in this trial.
    pub boots: u32,
    pub max_boots: u32,
}

/// What the init role calls to go back.  Implemented by the updater crate
/// (`ctrl` record rewrite); it must only return `Ok` once the previous bank
/// is committed as active, replicated, and re-read.
pub type RollbackHook = fn(&TrialRecord) -> io::Result<()>;

pub fn record_path(own_dir: &Path) -> PathBuf {
    own_dir.join("trial")
}

impl TrialRecord {
    pub fn new(previous_bank: u8) -> Self {
        TrialRecord { previous_bank, boots: 0, max_boots: DEFAULT_MAX_BOOTS }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut pending = false;
        let mut record = TrialRecord::new(0);
        let mut have_bank = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "pending" => pending = value == "1",
                "previous_bank" => {
                    record.previous_bank = value.parse().ok().filter(|b| *b <= 1)?;
                    have_bank = true;
                }
                "boots" => record.boots = value.parse().ok()?,
                "max_boots" => record.max_boots = value.parse().ok()?,
                _ => {}
            }
        }
        (pending && have_bank).then_some(record)
    }

    pub fn to_text(&self) -> String {
        format!(
            "pending=1\nprevious_bank={}\nboots={}\nmax_boots={}\n",
            self.previous_bank, self.boots, self.max_boots
        )
    }

    pub fn exhausted(&self) -> bool {
        self.boots > self.max_boots
    }
}

/// The pending trial, if any.
pub fn load(own_dir: &Path) -> Option<TrialRecord> {
    TrialRecord::parse(&std::fs::read_to_string(record_path(own_dir)).ok()?)
}

pub fn store(own_dir: &Path, record: &TrialRecord) -> io::Result<()> {
    std::fs::create_dir_all(own_dir)?;
    crate::paths::write_atomic(&record_path(own_dir), record.to_text().as_bytes(), 0o644)
}

/// Remove the record: the trial is confirmed, or abandoned after rollback.
pub fn clear(own_dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(record_path(own_dir)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// For the updater: start a trial after committing a new bank.
pub fn begin(own_dir: &Path, previous_bank: u8) -> io::Result<()> {
    store(own_dir, &TrialRecord::new(previous_bank))
}

/// Count this boot against a pending trial.  Returns the updated record.
pub fn count_boot(own_dir: &Path) -> Option<TrialRecord> {
    let mut record = load(own_dir)?;
    record.boots += 1;
    if let Err(e) = store(own_dir, &record) {
        log::warn!("trial record: {e}");
    }
    Some(record)
}

/// Whether a kernel command line and an MTD table (`/proc/mtd`) say that
/// the root file system is one of the two firmware banks.  A system
/// started from RAM (`dphone ramboot`: `root=mtd:ramroot`; or a test
/// kernel with its root in `userdata`) is from neither.
///
/// The D80 has one installed firmware and no banks: its root file system
/// partition ([`super::nvdata::D80_ROOT`]) counts as the installed one.
pub fn root_is_a_bank(cmdline: &str, proc_mtd: &str) -> bool {
    if cmdline.split_ascii_whitespace().any(|w| w.strip_prefix("root=") == Some(super::nvdata::D80_ROOT)) {
        return true;
    }
    opendeskphone_update::bank::root_mtd_index(cmdline).is_some_and(|index| {
        ["root0", "root1"].iter().any(|name| super::nvdata::mtd_index(proc_mtd, name) == Some(index))
    })
}

/// Whether this system was started from RAM rather than booted from a
/// bank.  Such a system is not the image a pending trial is about: it
/// must neither count against the trial nor confirm it.
pub fn started_from_ram() -> bool {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    !root_is_a_bank(&read("/proc/cmdline"), &read("/proc/mtd"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_or_ram() {
        let mtd = "dev:    size   erasesize  name\n\
                   mtd0: 00020000 00020000 \"boot1\"\n\
                   mtd3: 00ee0000 00020000 \"nvdata\"\n\
                   mtd6: 02d00000 00020000 \"root0\"\n\
                   mtd7: 02d00000 00020000 \"root1\"\n\
                   mtd8: 01000000 00020000 \"userdata\"\n";
        assert!(root_is_a_bank("console=ttyAMA0 root=/dev/mtdblock6 rootfstype=jffs2 ro", mtd));
        assert!(root_is_a_bank("root=/dev/mtdblock7 ro", mtd));
        // A root in userdata, and the RAM root of 'dphone ramboot'.
        assert!(!root_is_a_bank("root=/dev/mtdblock8 ro", mtd));
        assert!(!root_is_a_bank("root=mtd:ramroot rootfstype=jffs2 ro phram.phram=ramroot,0x6ec0000,0x1140000,0x20000", mtd));
        assert!(!root_is_a_bank("", mtd));
        // The D80: its one root file system partition, and no MTD table.
        assert!(root_is_a_bank("console=ttymxc0,115200 root=/dev/mmcblk0p5 rootfstype=squashfs ro rootwait", ""));
        assert!(!root_is_a_bank("console=ttymxc0,115200 root=/dev/mmcblk0p4 ro", ""));
        // With the RAM device registered first, the flash partitions count
        // from one: the bank is still found by name.
        let shifted = "mtd0: 01140000 00020000 \"ramroot\"\nmtd7: 02d00000 00020000 \"root0\"\nmtd8: 02d00000 00020000 \"root1\"\n";
        assert!(root_is_a_bank("root=/dev/mtdblock7 ro", shifted));
        assert!(!root_is_a_bank("root=/dev/mtdblock6 ro", shifted));
    }

    /// The display is covered while a trial is pending.  The cover must
    /// not end before the init role has ended an unhealthy trial boot (the
    /// user would see the idle screen and then a restart), and it gives up
    /// right after that, when nothing is going to end the trial.
    #[test]
    fn the_trial_cover_lasts_as_long_as_a_trial_boot_may() {
        assert_eq!(opendeskphone_ui::TRIAL_COVER_MAX_SECS, TRIAL_HEALTHY_SECS);
    }

    #[test]
    fn a_record_is_about_the_bank_that_is_not_the_one_to_return_to() {
        let r = TrialRecord::new(1);
        assert!(is_about(&r, Some(0)));
        assert!(!is_about(&r, Some(1)));
        assert!(!is_about(&r, None));
        // The early stage decides the same.
        let t = opendeskphone_ctrl::trial::Trial { previous_bank: 1, trial_bank: None, boots: 0, max_boots: 3 };
        for running in [Some(0), Some(1), None] {
            let counted = opendeskphone_ctrl::boot::step(running, Some(t)) != opendeskphone_ctrl::boot::Step::Boot;
            assert_eq!(counted, is_about(&r, running));
        }
    }

    #[test]
    fn lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), None);
        assert_eq!(count_boot(dir.path()), None);
        begin(dir.path(), 1).unwrap();
        for boot in 1..=DEFAULT_MAX_BOOTS {
            let r = count_boot(dir.path()).unwrap();
            assert_eq!((r.previous_bank, r.boots), (1, boot));
            assert!(!r.exhausted());
        }
        assert!(count_boot(dir.path()).unwrap().exhausted());
        clear(dir.path()).unwrap();
        clear(dir.path()).unwrap();
        assert_eq!(load(dir.path()), None);
    }

    #[test]
    fn parse_rejects_bad_records() {
        assert_eq!(TrialRecord::parse(""), None);
        assert_eq!(TrialRecord::parse("pending=0\nprevious_bank=1\n"), None);
        assert_eq!(TrialRecord::parse("pending=1\n"), None);
        assert_eq!(TrialRecord::parse("pending=1\nprevious_bank=7\n"), None);
        let r = TrialRecord::parse("pending=1\nprevious_bank=0\nboots=2\nmax_boots=4\n").unwrap();
        assert_eq!(r, TrialRecord { previous_bank: 0, boots: 2, max_boots: 4 });
        assert_eq!(TrialRecord::parse(&r.to_text()), Some(r));
    }
}
