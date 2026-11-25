//! Installing an image on a D80, for [`crate::Updater`]: the package is
//! left for the recovery.
//!
//! A D80 has one kernel and one root file system on its eMMC, and no spare
//! bank to write while the system runs.  Its image is an Android OTA
//! package in the EFF envelope, and what installs a package is the
//! recovery: a second small system in the eMMC's second partition, which
//! the boot loader starts in place of the first when `bootcmd` in its
//! environment says so (`opendeskphone-recovery`).  So nothing is installed
//! here.  `leave_for_recovery` does what the stock firmware does before
//! it restarts into its recovery, and one thing more:
//!
//! | Step | If power is lost right after |
//! | --- | --- |
//! | 1. The update lock; the envelope's model; an earlier command removed | the system starts |
//! | 2. The package stored in nvdata (the stock `/cache`), as `recovery/update.zip` | the system starts; the file is replaced by the next update |
//! | 3. The package checked: its layout; its recovery image | the same |
//! | 4. The recovery image written to the second partition, if it is not there already, and read back | the system starts (a recovery half written is not started) |
//! | 5. `recovery/command` names the package | the system starts; the command is replaced by the next update |
//! | 6. `bootcmd` set to start the recovery | the recovery starts and installs the package |
//!
//! Step 4 is what the stock firmware has no need of.  The recovery that
//! installs a package is the one that the package brings:
//!
//! - One of our packages has the recovery image in it (`recovery.img`),
//!   so an update that changes the recovery is installed by the new one.
//!   A package that brings no recovery image is refused here, where that
//!   can still be reported.
//! - A stock package has the stock recovery as a patch of its boot image,
//!   which is applied here (`opendeskphone_recovery::imgdiff`).  The
//!   stock recovery then verifies the package with the vendor's key and
//!   installs the stock firmware, so this is the way back to stock.
//!   Neither kind of package is verified here: the vendor's key is not in
//!   this firmware, and Open Desk Phone does not gate installs on
//!   signatures.
//!
//! A web upload is stored first (`stage`), and left for the recovery
//! (`commit`) when the user installs it.
//!
//! The recovery restarts into the system when it is done, whether or not
//! the package was installed, and leaves the result in
//! `recovery/last_install` ([`status`]).  Of a package it installed, it
//! records the checksum in `recovery/installed` ([`installed`]), which
//! provisioning compares with the checksum of the image that it offers.
//! There is no trial boot: a D80 has no other system to return to.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use eff::EffReader;
use opendeskphone_recovery::{command, env, imgdiff, sig, zip};
use opendeskphone_state::UpdatePhase;

use crate::error::{Result, UpdateError};
use crate::install::is_stock_version;
use crate::lock;

/// The files of a stock package that its recovery image is made of.
const STOCK_BOOT: &str = "boot.img";
const STOCK_PATCH: &str = "recovery/recovery-from-boot.p";
const STOCK_RESOURCES: &str = "system/etc/recovery-resource.dat";
/// What a recovery runs.
const UPDATE_BINARY: &str = opendeskphone_recovery::install::UPDATE_BINARY;
/// A web upload's package, while it waits to be installed.
const STAGED: &str = "staged.zip";

/// Where a D80 has what an update touches.
#[derive(Clone, Debug)]
pub struct Places {
    /// The eMMC, for the boot loader's environment.
    pub disk: PathBuf,
    /// The partition of the recovery image.
    pub recovery: PathBuf,
    /// nvdata, which the recovery knows as `/cache`.
    pub nvdata: PathBuf,
}

impl Places {
    /// The places on a phone whose nvdata is mounted at `nvdata`.
    pub fn phone(nvdata: &Path) -> Places {
        Places {
            disk: opendeskphone_recovery::DISK.into(),
            recovery: opendeskphone_recovery::RECOVERY_PARTITION.into(),
            nvdata: nvdata.into(),
        }
    }

    /// The places on a phone whose own directory in nvdata
    /// (`/nvdata/opendeskphone`) is `own_dir`.
    pub fn for_own_dir(own_dir: &Path) -> Places {
        Places::phone(own_dir.parent().unwrap_or(own_dir))
    }

    fn dir(&self) -> PathBuf {
        self.nvdata.join(command::DIR)
    }
}

fn image(text: impl Into<String>) -> UpdateError {
    UpdateError::Image(text.into())
}

fn at(path: &Path) -> impl Fn(std::io::Error) -> UpdateError + '_ {
    move |e| UpdateError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// Writes a file under another name, and gives it its own when all of it
/// is on the eMMC.
fn write_whole(path: &Path, data: &[u8]) -> Result<()> {
    let part = path.with_extension("part");
    let mut file = File::create(&part).map_err(at(&part))?;
    file.write_all(data).map_err(at(&part))?;
    file.sync_all().map_err(at(&part))?;
    std::fs::rename(&part, path).map_err(at(path))?;
    Ok(())
}

/// Writes the recovery image to its partition, unless it is there, and
/// reads it back.  Returns whether it was written.
fn write_recovery(partition: &Path, image: &[u8]) -> Result<bool> {
    let file = OpenOptions::new().read(true).write(true).open(partition).map_err(at(partition))?;
    let mut there = vec![0u8; image.len()];
    file.read_exact_at(&mut there, 0).map_err(at(partition))?;
    if there == image {
        return Ok(false);
    }
    file.write_all_at(image, 0).map_err(at(partition))?;
    file.sync_all().map_err(at(partition))?;
    file.read_exact_at(&mut there, 0).map_err(at(partition))?;
    if there != image {
        return Err(UpdateError::Verify(format!("{}: the recovery image read back is not the one written", partition.display())));
    }
    Ok(true)
}

/// The size of a partition (or, in a test, of the file that stands for it).
fn size_of(partition: &Path) -> Result<u64> {
    use std::io::Seek;
    File::open(partition).and_then(|mut f| f.seek(std::io::SeekFrom::End(0))).map_err(at(partition))
}

/// An image on its way in: its envelope opened and its label read.
pub(crate) struct Incoming<R: Read> {
    eff: EffReader<R>,
    pub model: String,
    pub version: String,
    pub is_stock: bool,
}

/// Opens an image for a D80, and refuses one for another model.
pub(crate) fn open<R: Read>(reader: R) -> Result<Incoming<R>> {
    let eff = EffReader::new(reader)?;
    let header = eff.header().clone();
    if !header.model.trim().eq_ignore_ascii_case("D80") {
        return Err(UpdateError::WrongModel { image: header.model, phone: "D80".into() });
    }
    let is_stock = is_stock_version(&header.version);
    Ok(Incoming { eff, model: header.model, version: header.version, is_stock })
}

/// A package left for the recovery, which the next restart starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Left {
    /// The recovery partition was written (it had another recovery).
    pub recovery_written: bool,
    /// The package's length.
    pub package_len: u64,
}

/// Stores an image's package in nvdata, writes the recovery that the
/// package brings, and has the boot loader start that recovery at the next
/// restart.  See the module text for the steps.  On an error the next
/// restart is the system's, and an earlier package left for the recovery
/// is gone.
pub(crate) fn leave_for_recovery<R: Read>(incoming: Incoming<R>, places: &Places, progress: &mut dyn FnMut(&UpdatePhase)) -> Result<Left> {
    // No command names a package while the package is being replaced.
    let dir = places.dir();
    std::fs::create_dir_all(&dir).map_err(at(&dir))?;
    let command_file = dir.join(command::COMMAND);
    match std::fs::remove_file(&command_file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(at(&command_file)(e)),
        _ => {}
    }
    let _ = std::fs::remove_file(dir.join(command::PACKAGE));
    let part = dir.join(command::PACKAGE).with_extension("part");
    let (package_len, recovery) = receive(incoming, &part, places, progress)?;
    let recovery_written = leave(&part, &recovery, places, progress).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })?;
    Ok(Left { recovery_written, package_len })
}

/// Stores an image's package in nvdata to be left for the recovery later
/// ([`commit`]), as a web upload waits for the user to install it.
/// Returns the package's length.
pub(crate) fn stage<R: Read>(incoming: Incoming<R>, places: &Places, progress: &mut dyn FnMut(&UpdatePhase)) -> Result<u64> {
    let dir = places.dir();
    std::fs::create_dir_all(&dir).map_err(at(&dir))?;
    let staged = dir.join(STAGED);
    let part = staged.with_extension("part");
    let (package_len, _) = receive(incoming, &part, places, progress)?;
    std::fs::rename(&part, &staged).map_err(at(&staged))?;
    Ok(package_len)
}

/// Leaves a package that [`stage`] stored for the recovery.  The package
/// is checked again, since the update lock was not held while it waited.
pub(crate) fn commit(package_len: u64, is_stock: bool, places: &Places, progress: &mut dyn FnMut(&UpdatePhase)) -> Result<Left> {
    let staged = places.dir().join(STAGED);
    let mut file = File::open(&staged).map_err(|_| image("the uploaded image is not there any more; upload it again"))?;
    if file.metadata().map_err(at(&staged))?.len() != package_len {
        return Err(image("the uploaded image is not the one that was checked; upload it again"));
    }
    let recovery = recovery_image(&mut file, package_len, is_stock, places)?;
    drop(file);
    let recovery_written = leave(&staged, &recovery, places, progress)?;
    Ok(Left { recovery_written, package_len })
}

/// Removes a package that [`stage`] stored.
pub(crate) fn discard(places: &Places) {
    let _ = std::fs::remove_file(places.dir().join(STAGED));
}

/// Receives an image's package into `part`, and checks it.  Returns the
/// package's length and the recovery that installs it.  On an error `part`
/// is removed.
fn receive<R: Read>(incoming: Incoming<R>, part: &Path, places: &Places, progress: &mut dyn FnMut(&UpdatePhase)) -> Result<(u64, Vec<u8>)> {
    let Incoming { mut eff, is_stock, .. } = incoming;
    let result = (|| {
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(part).map_err(at(part))?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut len = 0u64;
        loop {
            let n = match eff.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            file.write_all(&buf[..n]).map_err(at(part))?;
            len += n as u64;
            // The length is not known beforehand: megabytes, not a share.
            progress(&UpdatePhase::Receiving { percent: ((len >> 20) % 100) as u8 });
        }
        progress(&UpdatePhase::Checking);
        // Every image consists of the package followed by its signature.
        // The signature exists for the stock updater's benefit; this
        // reader only strips it, since the package's end sits just before
        // it.
        let end = len
            .checked_sub(sig::IMAGE_SIGNATURE)
            .filter(|end| sig::has_footer(&mut file, *end));
        let Some(len) = end else {
            return Err(image("the image does not contain a signed package followed by its signature"));
        };
        file.set_len(len).map_err(at(part))?;
        let recovery = recovery_image(&mut file, len, is_stock, places)?;
        file.sync_all().map_err(at(part))?;
        Ok((len, recovery))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

/// The recovery that installs a package of `len` bytes: the one that the
/// package brings, or for a stock package the stock recovery, made from
/// the package.
fn recovery_image(file: &mut File, len: u64, is_stock: bool, places: &Places) -> Result<Vec<u8>> {
    let limit = usize::try_from(size_of(&places.recovery)?).unwrap_or(usize::MAX);
    let mut archive = zip::Archive::open(file, len).map_err(image)?;
    if archive.entry(UPDATE_BINARY).is_none() {
        return Err(image(format!("the package has no {UPDATE_BINARY}")));
    }
    let recovery = if is_stock {
        let boot = archive.read(STOCK_BOOT, limit).map_err(image)?;
        let patch = archive.read(STOCK_PATCH, limit).map_err(image)?;
        let resources = archive.read(STOCK_RESOURCES, limit).map_err(image)?;
        imgdiff::stock_recovery(&boot, &patch, &resources, limit).map_err(image)?
    } else {
        archive.read(opendeskphone_recovery::RECOVERY_ENTRY, limit).map_err(image)?
    };
    if recovery.get(..8) != Some(b"ANDROID!") {
        return Err(image("the package's recovery image is not a boot image"));
    }
    Ok(recovery)
}

/// Leaves the package at `package` for the recovery: writes `recovery`,
/// which installs it, to its partition, moves the package to where the
/// command names it, and has the boot loader start the recovery.  Returns
/// whether the recovery partition was written.  On an error no command is
/// left.
fn leave(package: &Path, recovery: &[u8], places: &Places, progress: &mut dyn FnMut(&UpdatePhase)) -> Result<bool> {
    let dir = places.dir();
    let command_file = dir.join(command::COMMAND);
    let result = (|| {
        match std::fs::remove_file(&command_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(at(&command_file)(e)),
            _ => {}
        }
        progress(&UpdatePhase::Committing);
        let recovery_written = write_recovery(&places.recovery, recovery)?;
        let left = dir.join(command::PACKAGE);
        std::fs::rename(package, &left).map_err(at(&left))?;
        write_whole(&command_file, command::install_command().as_bytes())?;
        File::open(&dir).and_then(|d| d.sync_all()).map_err(at(&dir))?;
        env::set_bootcmd(&places.disk, env::BOOT_RECOVERY)
            .map_err(|e| UpdateError::Layout(format!("the boot loader's environment: {e}")))?;
        Ok(recovery_written)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&command_file);
    }
    result
}

/// The checksum of the package that the recovery last installed, which is
/// what the system runs; `None` when the recovery has recorded none.
pub fn installed(places: &Places) -> Option<String> {
    let text = std::fs::read_to_string(places.dir().join(command::INSTALLED)).ok()?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// Takes back a package that was left for the recovery and not installed
/// yet: the boot loader starts the system again, and the command and the
/// package are removed.  Returns whether there was anything to take back.
pub fn take_back(places: &Places, own_dir: &Path) -> Result<bool> {
    let _lock = lock::acquire(own_dir)?;
    let changed =
        env::set_bootcmd(&places.disk, env::BOOT_SYSTEM).map_err(|e| UpdateError::Layout(format!("the boot loader's environment: {e}")))?;
    let dir = places.dir();
    let mut removed = false;
    for name in [command::COMMAND, command::PACKAGE] {
        removed |= std::fs::remove_file(dir.join(name)).is_ok();
    }
    Ok(changed || removed)
}

/// What an update has left, and what the recovery said of the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The boot loader's `bootcmd`, or why it cannot be read.
    pub bootcmd: std::result::Result<String, String>,
    /// The boot loader starts the recovery at the next restart.
    pub recovery_next: bool,
    /// The package the command names, as the recovery will see it.
    pub command: Option<String>,
    /// The length of the package left in nvdata.
    pub package_len: Option<u64>,
    /// The package the recovery last ran with, and whether it installed
    /// it.
    pub last_install: Option<(String, bool)>,
}

pub fn status(places: &Places) -> Status {
    let dir = places.dir();
    let bootcmd = env::read(&places.disk).map(|env| env.get("bootcmd").unwrap_or_default());
    let text = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    Status {
        recovery_next: bootcmd.as_deref() == Ok(env::BOOT_RECOVERY),
        bootcmd,
        command: text(command::COMMAND).and_then(|t| command::parse(&t).update_package),
        package_len: std::fs::metadata(dir.join(command::PACKAGE)).ok().map(|m| m.len()),
        last_install: text(command::LAST_INSTALL).and_then(|t| command::parse_last_install(&t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eff::envelope::EffHeader;
    use zip::FileEntry;

    const KEY: &str = include_str!("../../opendeskphone-image/keys/d80-ota.key.pem");
    const PARTITION: usize = 64 * 1024;

    /// A phone's eMMC and nvdata in a directory: the environment with the
    /// system's `bootcmd`, and a recovery partition of 64 KiB.
    fn phone() -> (tempfile::TempDir, Places, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places { disk: dir.path().join("disk"), recovery: dir.path().join("p2"), nvdata: dir.path().join("nvdata") };
        let mut disk = vec![0u8; env::OFFSET as usize + env::SIZE];
        let mut block = vec![0u8; 4];
        block.extend_from_slice(b"bootdelay=3\0bootcmd=booti mmc3\0");
        block.resize(env::SIZE, 0);
        let sum = !eff::crc::crc(&block[4..]);
        block[..4].copy_from_slice(&sum.to_le_bytes());
        disk[env::OFFSET as usize..].copy_from_slice(&block);
        std::fs::write(&places.disk, disk).unwrap();
        std::fs::write(&places.recovery, vec![0x11u8; PARTITION]).unwrap();
        let own = places.nvdata.join("opendeskphone");
        std::fs::create_dir_all(&own).unwrap();
        (dir, places, own)
    }

    fn recovery_image() -> Vec<u8> {
        let mut image = b"ANDROID!".to_vec();
        image.extend_from_slice(&[0x5a; 3000]);
        image
    }

    fn signed(entries: &[FileEntry]) -> Vec<u8> {
        sig::sign(&zip::write(entries).unwrap(), KEY, sig::CERTIFICATE).unwrap()
    }

    fn ours() -> Vec<u8> {
        signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: "rootfs.img", data: &[7u8; 300_000], executable: false },
            FileEntry { name: "recovery.img", data: &recovery_image(), executable: false },
        ])
    }

    /// Builds a D80 image: the package in the envelope, followed by its
    /// signature.
    fn eff(model: &str, version: &str, package: &[u8]) -> Vec<u8> {
        sig::sign_eff(&EffHeader::new(model, version).unwrap(), package, KEY, [9; 8]).unwrap()
    }

    fn leave(image: &[u8], places: &Places) -> Result<Left> {
        leave_for_recovery(open(image)?, places, &mut |_| {})
    }

    #[test]
    fn our_package_is_left_with_its_recovery_written() {
        let (_dir, places, own) = phone();
        let package = ours();
        let left = leave(&eff("D80", "3_1_0", &package), &places).unwrap();
        assert_eq!(left, Left { recovery_written: true, package_len: package.len() as u64 });
        // The package, the command the recovery reads, and the recovery
        // image at the start of its partition, the rest of which is as it
        // was.
        assert_eq!(std::fs::read(places.nvdata.join("recovery/update.zip")).unwrap(), package);
        assert_eq!(std::fs::read_to_string(places.nvdata.join("recovery/command")).unwrap(), "--update_package=/cache/recovery/update.zip\n");
        let p2 = std::fs::read(&places.recovery).unwrap();
        let image = recovery_image();
        assert_eq!(p2.len(), PARTITION);
        assert_eq!(p2[..image.len()], image[..]);
        assert!(p2[image.len()..].iter().all(|b| *b == 0x11));
        let st = status(&places);
        assert_eq!(st.bootcmd.as_deref(), Ok("booti mmc3 recovery"));
        assert!(st.recovery_next);
        assert_eq!(st.command.as_deref(), Some("/cache/recovery/update.zip"));
        assert_eq!(st.package_len, Some(package.len() as u64));
        assert_eq!(st.last_install, None);

        // Again: the recovery is there already, and is not written.
        let left = leave(&eff("D80", "3_1_0", &package), &places).unwrap();
        assert!(!left.recovery_written);

        // Taken back: the system starts, and nothing is left to install.
        assert!(take_back(&places, &own).unwrap());
        let st = status(&places);
        assert_eq!((st.bootcmd.as_deref(), st.command, st.package_len), (Ok("booti mmc3"), None, None));
        assert!(!take_back(&places, &own).unwrap());
    }

    /// The phone as it was: the system's `bootcmd`, the recovery partition
    /// untouched, and nothing for a recovery to act on.
    fn untouched(places: &Places) {
        let st = status(places);
        assert_eq!(st.bootcmd.as_deref(), Ok("booti mmc3"));
        assert_eq!((st.command, st.package_len), (None, None));
        assert!(std::fs::read(&places.recovery).unwrap().iter().all(|b| *b == 0x11));
        assert!(!places.nvdata.join("recovery/update.part").exists());
    }

    #[test]
    fn what_the_recovery_would_not_install_is_refused() {
        let (_dir, places, _own) = phone();
        // An image for another model is refused before anything else is
        // checked.
        let container = [&eff::OTA_MAGIC.to_be_bytes()[..], &[0; 200]].concat();
        let other = eff::envelope::pack(&EffHeader::new("D60", "3_1_0").unwrap(), &container, [9; 8]).unwrap();
        assert!(matches!(leave(&other, &places), Err(UpdateError::WrongModel { .. })));
        untouched(&places);
        // Not laid out as a signed package.
        let unsigned = zip::write(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }]).unwrap();
        assert!(leave(&eff("D80", "3_1_0", &unsigned), &places).unwrap_err().to_string().contains("signed package"));
        untouched(&places);
        // No recovery image, one that is none, and one too large for the
        // partition.
        let none = signed(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }]);
        assert!(leave(&eff("D80", "3_1_0", &none), &places).unwrap_err().to_string().contains("recovery.img"));
        let not_one = signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: "recovery.img", data: b"not an image", executable: false },
        ]);
        assert!(leave(&eff("D80", "3_1_0", &not_one), &places).unwrap_err().to_string().contains("not a boot image"));
        let large = signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: "recovery.img", data: &[b'A'; PARTITION + 1], executable: false },
        ]);
        assert!(leave(&eff("D80", "3_1_0", &large), &places).is_err());
        untouched(&places);
        // Not an EFF image at all.
        assert!(leave(b"PK\x03\x04 and so on", &places).is_err());
        untouched(&places);
    }

    #[test]
    fn a_failed_update_takes_an_earlier_one_with_it() {
        // The package of an earlier update is replaced by the next one's,
        // so its command must not outlive it.
        let (_dir, places, _own) = phone();
        leave(&eff("D80", "3_1_0", &ours()), &places).unwrap();
        assert!(leave(&eff("D80", "3_1_1", b"PK\x03\x04 and nothing of a package"), &places).is_err());
        let st = status(&places);
        assert_eq!((st.command, st.package_len), (None, None));
    }

    #[test]
    fn a_stock_package_is_left_with_the_stock_recovery() {
        const BOOT: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/boot.img");
        const PATCH: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/recovery-from-boot.p");
        const RESOURCES: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/recovery-resource.dat");
        let (_dir, places, _own) = phone();
        // Signed with a key the stock recovery would accept; any key
        // stands in here.  No signature is verified on this path: the
        // vendor's key is not in this firmware, and the image signature
        // is the stock updater's concern.
        let package = signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: STOCK_BOOT, data: BOOT, executable: false },
            FileEntry { name: STOCK_PATCH, data: PATCH, executable: false },
            FileEntry { name: STOCK_RESOURCES, data: RESOURCES, executable: false },
        ]);
        assert!(open(&eff("D80", "1_12_14", &package)[..]).unwrap().is_stock);
        let left = leave(&eff("D80", "1_12_14", &package), &places).unwrap();
        assert_eq!(left, Left { recovery_written: true, package_len: package.len() as u64 });
        // The stored package is the image minus its signature, and the
        // recovery written is the one the package's patch produces.
        assert_eq!(std::fs::read(places.nvdata.join("recovery/update.zip")).unwrap(), package);
        let want = imgdiff::stock_recovery(BOOT, PATCH, RESOURCES, PARTITION).unwrap();
        assert_eq!(std::fs::read(&places.recovery).unwrap()[..want.len()], want[..]);
        assert!(status(&places).recovery_next);

        // A stock package without its recovery's parts is refused.
        let (_dir, places, _own) = phone();
        let bare = signed(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }, FileEntry { name: STOCK_BOOT, data: BOOT, executable: false }]);
        assert!(leave(&eff("D80", "1_12_14", &bare), &places).unwrap_err().to_string().contains(STOCK_PATCH));
        untouched(&places);
    }

    #[test]
    fn an_uploaded_package_is_left_only_by_its_commit() {
        let (_dir, places, _own) = phone();
        let package = ours();
        let image = eff("D80", "3_1_0", &package);
        let upload = || open(&image[..]).unwrap();
        let len = stage(upload(), &places, &mut |_| {}).unwrap();
        assert_eq!(len, package.len() as u64);
        // While it waits, there is nothing for a recovery to act on.
        untouched(&places);
        let left = commit(len, false, &places, &mut |_| {}).unwrap();
        assert_eq!(left, Left { recovery_written: true, package_len: len });
        assert_eq!(std::fs::read(places.nvdata.join("recovery/update.zip")).unwrap(), package);
        assert!(status(&places).recovery_next);
        // A commit of an upload that is gone leaves alone the package left
        // before.
        assert!(commit(len, false, &places, &mut |_| {}).unwrap_err().to_string().contains("upload it again"));
        assert!(status(&places).command.is_some());
        stage(upload(), &places, &mut |_| {}).unwrap();
        discard(&places);
        assert!(commit(len, false, &places, &mut |_| {}).is_err());
    }

    #[test]
    fn the_updater_leaves_a_d80_image_for_the_recovery() {
        use crate::install::{Options, Target, Updater};
        use opendeskphone_state::{Model, Store, Update};
        fn on_d80<'a>(flash: &'a mut opendeskphone_hal::Null, own: &Path, places: &Places) -> Updater<'a> {
            Updater::new(flash, Model::D80, "", own).places(places.clone())
        }
        let (_dir, places, own) = phone();
        let image = eff("D80", "3_1_0", &ours());
        // A D80 has no MTD flash.
        let mut flash = opendeskphone_hal::Null("mtd");
        // A web upload: nothing is left until the commit.
        let store = Store::new(Model::D80);
        let staged = on_d80(&mut flash, &own, &places).store(store.clone()).stage_eff(&image[..]).unwrap();
        assert_eq!(store.get::<Update>().incoming_version, "3_1_0");
        assert!(matches!(staged.target, Target::Recovery { recovery_written: false, .. }), "{:?}", staged.target);
        untouched(&places);
        let done = on_d80(&mut flash, &own, &places).commit(&staged).unwrap();
        assert!(matches!(done.target, Target::Recovery { recovery_written: true, .. }), "{:?}", done.target);
        assert!(status(&places).recovery_next);
        // From a stream, as provisioning has it, and with options for banks.
        assert!(on_d80(&mut flash, &own, &places).install_eff(&image[..]).is_ok());
        let banks = Options { allow_unbootable_other: true, ..Options::default() };
        let refused = on_d80(&mut flash, &own, &places).options(banks).install_eff(&image[..]).unwrap_err();
        assert!(refused.to_string().contains("no firmware banks"), "{refused}");
    }
}
