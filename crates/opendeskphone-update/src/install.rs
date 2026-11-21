//! Installing an EFF into the bank that is not running.
//!
//! The order of operations, and the state boot2 sees after each:
//!
//! | Step | Flash state if power is lost right after |
//! | --- | --- |
//! | 0. The update lock; no trial pending; for a file, the whole image read and checked | untouched |
//! | 1. Checks (model, OTA header CRCs, sizes, kernel header, the kept bank is bootable) | untouched |
//! | 2. `ctrl`: target header invalidated, active side set to the kept bank | boots the kept bank |
//! | 3. Erase and write kernel, then root file system; each block read back and compared | boots the kept bank |
//! | 4. Stream CRCs, end of stream, whole-image read-back CRCs | boots the kept bank |
//! | 5. The trial record; `ctrl`: target header filled in, active side set to the target | boots the new image |
//!
//! Steps 2 and 5 rewrite `ctrl` one block at a time ([`crate::ctrl::write`]),
//! so a power cut inside either leaves boot2 with the record from before or
//! after the step, both of which are in the table.
//!
//! Two rules, and how they meet:
//!
//! - **Nothing is touched before the image is known to be good.**  An
//!   image in a file ([`Updater::install_eff_file`]) is read twice: once
//!   through every check (envelope, model, header CRCs, sizes, kernel
//!   header, both component CRCs, the end of the stream), with no flash
//!   access and no change to the trial record, and then again to write it.
//! - **The image is never held in memory or in `/tmp`.**  An image that
//!   arrives as a stream (standard input, a download, a web upload;
//!   [`Updater::install_eff`], [`Updater::stage_eff`]) cannot be read
//!   twice.  Everything that can be known from its start is checked before
//!   the first erase (envelope, model, header CRCs, sizes, the kernel's
//!   header); from then on only the bank that is not running is written,
//!   and that bank is not selectable (its `ctrl` header is invalid) until
//!   the stream's CRCs, the end of the stream, and the read-back of the
//!   whole bank have passed.  A stream that turns out truncated or corrupt
//!   has cost the spare bank its old contents and nothing else: the
//!   running bank, the selection, and the trial record are as they were.
//!
//! Memory use is one erase block (128 KiB) of data plus one of read-back.
//!
//! One update runs at a time ([`crate::lock`]), and none starts while an
//! installed image is still on trial ([`crate::trial`]): its bank is either
//! running or about to get its first boot, and the bank to return to is the
//! other one, so there is no bank to write.  [`abandon`] gives the trial up.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use opendeskphone_hal::Mtd;
use opendeskphone_state::{Model, Store, Update, UpdatePhase};
use eff::ctrl::{CtrlRecord, Side};
use eff::ota::{AppHeader, OtaReader};
use eff::EffReader;

use crate::bank::{self, Bank};
use crate::boot2;
use crate::ctrl::{self, CTRL_PART};
use crate::error::{Result, UpdateError};
use crate::flash::{self, OnBlockFailure};
use crate::lock::{self, UpdateLock};
use crate::trial;

/// Knobs; the defaults are right for the phone.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Bank to write.  Default: the one not running; when the running
    /// system is from neither bank (RAM boot), the one boot2 would not
    /// boot next.  Naming the running bank is refused.
    pub target: Option<Bank>,
    /// Mark a block bad when it fails to erase, program, or read back,
    /// and continue in the next block (stock's behavior on a failed
    /// write).  Off by default: bad-block marks are permanent and shrink
    /// the 2 MiB kernel slot, so a failure stops the update instead and
    /// the cause can be looked at.
    pub mark_bad_on_failure: bool,
    /// Go ahead even if the bank that stays is not bootable by boot2's
    /// rules (for recovery work from a RAM boot).
    pub allow_unbootable_other: bool,
}

/// A finished, committed installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The bank now selected for the next boot.
    pub bank: Bank,
    /// The bank that was kept; the rollback target.
    pub previous: Bank,
    /// Version from the EFF's encrypted header.
    pub version: String,
    /// The image is stock firmware (version below 3).
    pub is_stock: bool,
    /// The header now stored in `ctrl` for `bank`.
    pub header: AppHeader,
    /// Bad blocks skipped in the kernel and root partitions.
    pub bad_blocks: (u32, u32),
}

/// An image written to the spare bank and verified there, but not
/// selected: its `ctrl` header is still invalid, so boot2 cannot boot it.
/// [`Updater::commit`] selects it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub bank: Bank,
    pub previous: Bank,
    pub version: String,
    pub is_stock: bool,
    pub header: AppHeader,
    pub bad_blocks: (u32, u32),
}

/// Why nothing here installs an image on a D80.  Its image is an Android
/// OTA package, which its recovery writes to the eMMC; it has no flash
/// banks, no `ctrl` partition, and no boot2.  `d80` (with the feature of
/// that name) leaves a package for the recovery, and `dphone update
/// install` uses it; the updater of this module, which the web interface
/// and provisioning use, does not.
pub const D80_NOT_HERE: &str = "a D80's firmware is an Android OTA package, which the phone's recovery installs; \
     'dphone update install' leaves one for the recovery, and nothing else in this firmware does yet";

/// Reads a whole EFF through every check that needs no flash: the
/// envelope, the model, the OTA header and its CRCs, the kernel's header as
/// boot2 reads it, both component CRCs, and the end of the stream.
/// Returns the envelope's version.
pub fn check_eff<R: Read>(reader: R, model: Model) -> Result<String> {
    if model.is_touch() {
        return Err(UpdateError::Refused(D80_NOT_HERE.into()));
    }
    let eff = EffReader::new(reader)?;
    let header = eff.header().clone();
    let phone = model.to_string();
    if !header.model.trim().eq_ignore_ascii_case(&phone) {
        return Err(UpdateError::WrongModel { image: header.model, phone });
    }
    let mut ota = OtaReader::new(eff)?;
    let app = ota.header().app;
    if app.kernel_size == 0 || app.rootfs_size == 0 {
        return Err(UpdateError::Image("empty kernel or root file system".into()));
    }
    let mut kernel = ota.next_component().expect("kernel component");
    let mut head = vec![0u8; (app.kernel_size as usize).min(boot2::KERNEL_HEADER_LEN)];
    kernel.read_exact(&mut head)?;
    boot2::check_kernel_header(&head, app.kernel_size as u64).map_err(|p| UpdateError::Image(p.to_string()))?;
    std::io::copy(&mut kernel, &mut std::io::sink())?;
    kernel.finish()?;
    let mut rootfs = ota.next_component().expect("rootfs component");
    std::io::copy(&mut rootfs, &mut std::io::sink())?;
    rootfs.finish()?;
    ota.finish()?;
    Ok(header.version)
}

/// Whether a version string (`2_9_28`, `3_0_0`, `3.0.0`) is a stock
/// release: stock stopped at 2.x, Open Desk Phone starts at 3.
pub fn is_stock_version(version: &str) -> bool {
    let major: String = version.trim().chars().take_while(char::is_ascii_digit).collect();
    major.parse::<u32>().map_or(true, |m| m < 3)
}

type ProgressFn<'a> = Box<dyn FnMut(&UpdatePhase) + 'a>;

/// The updater.  One per installation.
pub struct Updater<'a> {
    mtd: &'a mut dyn Mtd,
    model: Model,
    cmdline: String,
    own_dir: PathBuf,
    options: Options,
    store: Option<Store>,
    progress: Option<ProgressFn<'a>>,
    last: UpdatePhase,
}

impl<'a> Updater<'a> {
    /// `cmdline` is the kernel command line (`/proc/cmdline`), from which the
    /// running bank is taken.  `own_dir` is `/nvdata/opendeskphone`: the
    /// update lock is taken on it and the trial record lives in it.
    pub fn new(mtd: &'a mut dyn Mtd, model: Model, cmdline: &str, own_dir: &Path) -> Self {
        Updater {
            mtd,
            model,
            cmdline: cmdline.to_string(),
            own_dir: own_dir.to_path_buf(),
            options: Options::default(),
            store: None,
            progress: None,
            last: UpdatePhase::Idle,
        }
    }

    pub fn options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Publish progress in the state store's `Update` topic.
    pub fn store(mut self, store: Store) -> Self {
        self.store = Some(store);
        self
    }

    /// Also call this on every progress change.
    pub fn progress(mut self, f: impl FnMut(&UpdatePhase) + 'a) -> Self {
        self.progress = Some(Box::new(f));
        self
    }

    fn phase(&mut self, phase: UpdatePhase) {
        if phase == self.last {
            return;
        }
        self.last = phase.clone();
        if let Some(f) = &mut self.progress {
            f(&phase);
        }
        if let Some(s) = &self.store {
            s.update::<Update, _>(|u| u.phase = phase);
        }
    }

    fn finished<T>(&mut self, result: Result<T>, done: UpdatePhase) -> Result<T> {
        match &result {
            Ok(_) => self.phase(done),
            Err(e) => {
                log::error!("firmware update failed: {e}");
                self.phase(UpdatePhase::Failed(e.to_string()));
            }
        }
        result
    }

    /// The update lock, and the refusal while an installed image is on
    /// trial.  Nothing is written.
    fn begin(&mut self) -> Result<UpdateLock> {
        if self.model.is_touch() {
            return Err(UpdateError::Refused(D80_NOT_HERE.into()));
        }
        let lock = lock::acquire(&self.own_dir)?;
        if let Some(t) = trial::load(&self.own_dir) {
            let mut part = self.mtd.open(CTRL_PART)?;
            let rec = ctrl::read(part.as_mut())?;
            if trial::is_pending(&t, &rec) {
                return Err(UpdateError::Refused(format!(
                    "the image in bank {:?} was installed and is not confirmed yet (it is confirmed when it has \
                     come up healthy after a reboot); installing another now would overwrite it or the bank to \
                     return to.  Reboot and let it come up, or give it up with 'dphone update abandon'",
                    t.previous_bank.other()
                )));
            }
        }
        Ok(lock)
    }

    /// Installs an EFF that arrives as a stream (our images and stock
    /// images alike) and selects it for the next boot, recording the trial.
    /// On error nothing is committed: the next boot is the bank that was
    /// kept, and the trial record is unchanged.  See the module text for
    /// what a bad stream costs; use [`Updater::install_eff_file`] when the
    /// image is in a file.
    pub fn install_eff<R: Read>(&mut self, reader: R) -> Result<Installed> {
        let result = (|| {
            let _lock = self.begin()?;
            let staged = self.stage(reader)?;
            self.commit_staged(&staged, false)
        })();
        self.finished(result, UpdatePhase::Complete)
    }

    /// Installs an EFF from a file: the whole file is read and checked
    /// first ([`check_eff`]), and only then is anything written.
    pub fn install_eff_file<R: Read + Seek>(&mut self, mut file: R) -> Result<Installed> {
        let result = (|| {
            let _lock = self.begin()?;
            self.phase(UpdatePhase::Checking);
            file.seek(SeekFrom::Start(0))?;
            check_eff(&mut file, self.model)?;
            file.seek(SeekFrom::Start(0))?;
            let staged = self.stage(&mut file)?;
            self.commit_staged(&staged, false)
        })();
        self.finished(result, UpdatePhase::Complete)
    }

    /// Writes an EFF that arrives as a stream into the spare bank and
    /// verifies it there, without selecting it (a web upload, which the
    /// user confirms afterwards).  The bank stays unselectable until
    /// [`Updater::commit`].
    pub fn stage_eff<R: Read>(&mut self, reader: R) -> Result<Staged> {
        let result = (|| {
            let _lock = self.begin()?;
            self.stage(reader)
        })();
        // Not `Complete`: nothing is installed until the commit.
        self.finished(result, UpdatePhase::Idle)
    }

    /// Selects a staged image for the next boot and records the trial.  The
    /// bank is read back and compared with the image's CRCs once more,
    /// because the update lock was not held since it was staged.
    pub fn commit(&mut self, staged: &Staged) -> Result<Installed> {
        let result = (|| {
            let _lock = self.begin()?;
            self.commit_staged(staged, true)
        })();
        self.finished(result, UpdatePhase::Complete)
    }

    fn stage<R: Read>(&mut self, reader: R) -> Result<Staged> {
        self.phase(UpdatePhase::Checking);
        let eff = EffReader::new(reader)?;
        let header = eff.header().clone();
        let phone = self.model.to_string();
        if !header.model.trim().eq_ignore_ascii_case(&phone) {
            return Err(UpdateError::WrongModel { image: header.model, phone });
        }
        let is_stock = is_stock_version(&header.version);
        if let Some(s) = &self.store {
            s.update::<Update, _>(|u| {
                u.incoming_version = header.version.clone();
                u.incoming_is_stock = is_stock;
            });
        }
        log::info!(
            "firmware update: {} {}{}",
            header.model,
            header.version,
            if is_stock { " (stock firmware)" } else { "" }
        );
        let version = header.version;

        // The 160-byte header: magic, type 0, and both header CRCs.
        let mut ota = OtaReader::new(eff)?;
        let app = ota.header().app;
        if app.kernel_size == 0 || app.rootfs_size == 0 {
            return Err(UpdateError::Image("empty kernel or root file system".into()));
        }

        // Where things are, by name; nothing here knows an offset.
        let parts = self.mtd.partitions()?;
        let running = bank::running_bank(&self.cmdline, &parts);
        let mut ctrl_part = self.mtd.open(CTRL_PART)?;
        let repl = ctrl::replication_for(self.model.family());
        let mut rec = ctrl::read(ctrl_part.as_mut())?;
        let target = match (self.options.target, running) {
            (Some(t), Some(r)) if t == r => {
                return Err(UpdateError::Refused(format!("bank {t:?} holds the running system")));
            }
            (Some(t), _) => t,
            (None, Some(r)) => r.other(),
            (None, None) => bank::from_index(rec.boot_side()).unwrap_or(Bank::A).other(),
        };
        let keep = target.other();
        let (ti, ki) = (bank::index(target), bank::index(keep));
        let mut kpart = self.mtd.open(bank::kernel_part(target))?;
        let mut rpart = self.mtd.open(bank::rootfs_part(target))?;

        // The bank that stays must be one boot2 will boot once the target
        // is invalid: header valid and a loadable kernel.
        if !self.options.allow_unbootable_other {
            if !rec.sides[ki].is_valid() {
                return Err(UpdateError::Refused(format!(
                    "bank {keep:?} has no valid header in ctrl; updating would leave no bootable bank"
                )));
            }
            let mut other_k = self.mtd.open(bank::kernel_part(keep))?;
            if let Err(p) = boot2::check_stored_kernel(other_k.as_mut())? {
                return Err(UpdateError::Refused(format!("bank {keep:?} is not bootable: {p}")));
            }
        }

        // Sizes against what the partitions can hold after bad blocks.
        let (kcap, kbad) = flash::capacity(kpart.as_mut())?;
        let (rcap, rbad) = flash::capacity(rpart.as_mut())?;
        if app.kernel_size as u64 > kcap {
            return Err(UpdateError::Layout(format!(
                "kernel is {} bytes; {} holds {kcap} ({kbad} bad blocks)",
                app.kernel_size,
                kpart.info().name
            )));
        }
        if app.rootfs_size as u64 > rcap {
            return Err(UpdateError::Layout(format!(
                "root file system is {} bytes; {} holds {rcap} ({rbad} bad blocks)",
                app.rootfs_size,
                rpart.info().name
            )));
        }

        // The start of the kernel, before anything is touched: boot2 loads
        // by the image's own header and hangs or overwrites itself on a
        // bad one, and no CRC would catch that.
        let mut kernel = ota.next_component().expect("kernel component");
        let head_len = (app.kernel_size as usize).min(kpart.info().erase_size as usize);
        let mut head = vec![0u8; head_len];
        kernel.read_exact(&mut head)?;
        boot2::check_kernel_header(&head, app.kernel_size as u64)
            .map_err(|p| UpdateError::Image(p.to_string()))?;

        let on_failure =
            if self.options.mark_bad_on_failure { OnBlockFailure::MarkBad } else { OnBlockFailure::Fail };

        // Step 2: from here on the target is invalid until committed.
        rec.sides[ti] = Side::INVALID;
        rec.active = ki as u32;
        ctrl::write(ctrl_part.as_mut(), &rec, repl)?;
        let now = ctrl::read(ctrl_part.as_mut())?;
        if now.sides[ti].is_valid() || (now.sides[ki].is_valid() && now.boot_side() != ki as u32) {
            return Err(UpdateError::Ctrl("target bank is still selectable after invalidation".into()));
        }

        // Step 3: kernel.
        self.phase(UpdatePhase::Erasing { percent: 0 });
        flash::erase_all(kpart.as_mut(), on_failure, |_, _| {})?;
        self.phase(UpdatePhase::WritingKernel { percent: 0 });
        let kskipped = flash::program(kpart.as_mut(), &head, &mut kernel, app.kernel_size as u64, on_failure, |d, t| {
            self.phase(UpdatePhase::WritingKernel { percent: flash::percent(d, t) })
        })?;
        drop(head);
        kernel.finish()?;

        // Root file system.
        let mut rootfs = ota.next_component().expect("rootfs component");
        flash::erase_all(rpart.as_mut(), on_failure, |d, t| {
            self.phase(UpdatePhase::Erasing { percent: flash::percent(d, t) })
        })?;
        self.phase(UpdatePhase::WritingRootfs { percent: 0 });
        let rskipped = flash::program(rpart.as_mut(), &[], &mut rootfs, app.rootfs_size as u64, on_failure, |d, t| {
            self.phase(UpdatePhase::WritingRootfs { percent: flash::percent(d, t) })
        })?;
        rootfs.finish()?;

        // Step 4: the stream must end here (this also checks the EFF's
        // final padding), and the flash must read back as the image.
        ota.finish()?;
        drop((kpart, rpart, ctrl_part));
        self.read_back(target, &app)?;
        Ok(Staged {
            bank: target,
            previous: keep,
            version,
            is_stock,
            header: app,
            bad_blocks: (kbad.max(kskipped), rbad.max(rskipped)),
        })
    }

    /// Reads the whole bank back and compares it with the image
    /// ([`flash::verify_image`]).
    fn read_back(&mut self, target: Bank, app: &AppHeader) -> Result<()> {
        let mut kpart = self.mtd.open(bank::kernel_part(target))?;
        let mut rpart = self.mtd.open(bank::rootfs_part(target))?;
        self.phase(UpdatePhase::Verifying { percent: 0 });
        flash::verify_image(kpart.as_mut(), rpart.as_mut(), app, |p| self.phase(UpdatePhase::Verifying { percent: p }))
    }

    /// Step 5.  With `reverify`, the bank is read back again first.
    fn commit_staged(&mut self, staged: &Staged, reverify: bool) -> Result<Installed> {
        let (target, keep) = (staged.bank, staged.previous);
        let (ti, ki) = (bank::index(target), bank::index(keep));
        let parts = self.mtd.partitions()?;
        if bank::running_bank(&self.cmdline, &parts) == Some(target) {
            return Err(UpdateError::Refused(format!("bank {target:?} holds the running system")));
        }
        if reverify {
            self.read_back(target, &staged.header)?;
        }
        let mut ctrl_part = self.mtd.open(CTRL_PART)?;
        let mut rec = ctrl::read(ctrl_part.as_mut())?;
        if !self.options.allow_unbootable_other && !rec.sides[ki].is_valid() {
            return Err(UpdateError::Refused(format!("bank {keep:?} has no valid header in ctrl")));
        }
        self.phase(UpdatePhase::Committing);
        // The trial record first: should the power fail between the two,
        // boot2 still boots the kept bank, and a record whose bank to
        // return to is the one running is not counted.  The other order
        // could leave the new image selected with nothing to go back by.
        if !staged.is_stock {
            trial::record(&self.own_dir, keep, target)
                .map_err(|e| UpdateError::Io(std::io::Error::new(e.kind(), format!("recording the trial boot: {e}"))))?;
        }
        rec.sides[ti] = Side::from_header(&staged.header);
        rec.active = ti as u32;
        let committed = (|| {
            ctrl::write(ctrl_part.as_mut(), &rec, ctrl::replication_for(self.model.family()))?;
            let now = ctrl::read(ctrl_part.as_mut())?;
            if now != rec || now.boot_side() != ti as u32 {
                return Err(UpdateError::Ctrl("committed record does not read back".into()));
            }
            Ok(())
        })();
        if let Err(e) = committed {
            // Take the record back only if boot2 is sure not to boot the
            // new bank; else leave it to count.
            if ctrl::read(ctrl_part.as_mut()).is_ok_and(|now| now.boot_side() == ki as u32) {
                let _ = trial::clear(&self.own_dir);
            }
            return Err(e);
        }
        if staged.is_stock {
            // Stock does not know the record; one left behind would be
            // counted against whatever Open Desk Phone image comes next.
            let _ = trial::clear(&self.own_dir);
        }
        log::info!("firmware update: bank {target:?} written and selected ({})", staged.version);
        Ok(Installed {
            bank: target,
            previous: keep,
            version: staged.version.clone(),
            is_stock: staged.is_stock,
            header: staged.header,
            bad_blocks: staged.bad_blocks,
        })
    }
}

/// Makes `bank` the next boot without writing any image: sets the active
/// side in `ctrl`.  Refused unless boot2 would then really boot it (valid
/// header and a loadable kernel).  This is the rollback primitive.
/// Callers other than the init role hold the update lock
/// ([`crate::lock::acquire`]).
pub fn select_bank(mtd: &mut dyn Mtd, model: Model, bank: Bank) -> Result<()> {
    let mut ctrl_part = mtd.open(CTRL_PART)?;
    let mut rec = ctrl::read(ctrl_part.as_mut())?;
    let i = bank::index(bank);
    if !rec.sides[i].is_valid() {
        return Err(UpdateError::Refused(format!("bank {bank:?} has no valid header in ctrl")));
    }
    if let Err(p) = boot2::check_stored_kernel(mtd.open(bank::kernel_part(bank))?.as_mut())? {
        return Err(UpdateError::Refused(format!("bank {bank:?} is not bootable: {p}")));
    }
    rec.active = i as u32;
    ctrl::write(ctrl_part.as_mut(), &rec, ctrl::replication_for(model.family()))?;
    let now = ctrl::read(ctrl_part.as_mut())?;
    if now.boot_side() != i as u32 {
        return Err(UpdateError::Ctrl("bank selection does not read back".into()));
    }
    Ok(())
}

/// Gives up an installed image that is not confirmed yet: selects the bank
/// the trial record names as the one to return to, and removes the record.
/// Returns that bank, or `None` when no trial was recorded.  The phone
/// must be restarted if it is running the image given up.
pub fn abandon(mtd: &mut dyn Mtd, model: Model, own_dir: &Path) -> Result<Option<Bank>> {
    let _lock = lock::acquire(own_dir)?;
    let Some(t) = trial::load(own_dir) else { return Ok(None) };
    let rec: CtrlRecord = ctrl::read(mtd.open(CTRL_PART)?.as_mut())?;
    if rec.boot_side() != bank::index(t.previous_bank) as u32 {
        select_bank(mtd, model, t.previous_bank)?;
    }
    trial::clear(own_dir)?;
    Ok(Some(t.previous_bank))
}

#[cfg(test)]
mod tests {
    use super::is_stock_version;

    #[test]
    fn stock_versions() {
        assert!(is_stock_version("2_9_28"));
        assert!(is_stock_version("1_4_0"));
        assert!(is_stock_version("2.9.28"));
        assert!(!is_stock_version("3_0_0"));
        assert!(!is_stock_version("3.1.0"));
        assert!(!is_stock_version("10_0_0"));
        // Unparseable: treat as foreign.
        assert!(is_stock_version("custom"));
    }
}
