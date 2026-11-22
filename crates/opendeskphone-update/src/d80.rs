//! Installing an image on a D80: the package is left for the recovery.
//!
//! A D80 has one kernel and one root file system on its eMMC, and no spare
//! bank to write while the system runs.  Its image is an Android OTA
//! package in the EFF envelope, and what installs a package is the
//! recovery: a second small system in the eMMC's second partition, which
//! the boot loader starts in place of the first when `bootcmd` in its
//! environment says so (`opendeskphone-recovery`).  So nothing is installed
//! here.  [`leave_for_recovery`] does what the stock firmware does before
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
//! The recovery restarts into the system when it is done, whether or not
//! the package was installed, and leaves the result in
//! `recovery/last_install` ([`status`]).  There is no trial boot: a D80 has
//! no other system to return to.

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

    fn dir(&self) -> PathBuf {
        self.nvdata.join(command::DIR)
    }
}

/// A package left for the recovery, which the next restart starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Left {
    /// Version from the EFF's encrypted header.
    pub version: String,
    /// The image is stock firmware, and the recovery written is stock's.
    pub is_stock: bool,
    /// The recovery partition was written (it had another recovery).
    pub recovery_written: bool,
    /// The package's length.
    pub package_len: u64,
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

/// Takes an EFF image for a D80 from `reader`, stores its package in
/// nvdata, writes the recovery that the package brings, and has the boot
/// loader start that recovery at the next restart.  See the module text
/// for the steps.  `own_dir` is `/nvdata/opendeskphone`, for the update
/// lock.  On an error the next restart is the system's, as before.
pub fn leave_for_recovery<R: Read>(reader: R, places: &Places, own_dir: &Path, mut progress: impl FnMut(&UpdatePhase)) -> Result<Left> {
    let _lock = lock::acquire(own_dir)?;
    progress(&UpdatePhase::Checking);
    let mut eff = EffReader::new(reader)?;
    let header = eff.header().clone();
    if !header.model.trim().eq_ignore_ascii_case("D80") {
        return Err(UpdateError::WrongModel { image: header.model, phone: "D80".into() });
    }
    let is_stock = is_stock_version(&header.version);
    log::info!("firmware update: {} {}{}", header.model, header.version, if is_stock { " (stock firmware)" } else { "" });

    // No command names a package while the package is being replaced.
    let dir = places.dir();
    std::fs::create_dir_all(&dir).map_err(at(&dir))?;
    let (command_file, package) = (dir.join(command::COMMAND), dir.join(command::PACKAGE));
    match std::fs::remove_file(&command_file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(at(&command_file)(e)),
        _ => {}
    }
    let _ = std::fs::remove_file(&package);

    // The package, as it comes.
    let part = package.with_extension("part");
    let result = (|| {
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&part).map_err(at(&part))?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut len = 0u64;
        loop {
            let n = match eff.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            file.write_all(&buf[..n]).map_err(at(&part))?;
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
        file.set_len(len).map_err(at(&part))?;
        let limit = size_of(&places.recovery)?;
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut archive = zip::Archive::open(&mut file, len).map_err(image)?;
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
        file.sync_all().map_err(at(&part))?;
        drop(file);

        progress(&UpdatePhase::Committing);
        let recovery_written = write_recovery(&places.recovery, &recovery)?;
        std::fs::rename(&part, &package).map_err(at(&package))?;
        write_whole(&command_file, command::install_command().as_bytes())?;
        File::open(&dir).and_then(|d| d.sync_all()).map_err(at(&dir))?;
        env::set_bootcmd(&places.disk, env::BOOT_RECOVERY)
            .map_err(|e| UpdateError::Layout(format!("the boot loader's environment: {e}")))?;
        Ok(Left { version: header.version.clone(), is_stock, recovery_written, package_len: len })
    })();
    match &result {
        Ok(left) => {
            log::info!(
                "firmware update: {} is left for the recovery ({} bytes{}); it is installed at the next restart",
                left.version,
                left.package_len,
                if left.recovery_written { ", recovery image written" } else { "" }
            );
            progress(&UpdatePhase::Complete);
        }
        Err(e) => {
            // Nothing is left that a recovery would act on.
            let _ = std::fs::remove_file(&command_file);
            let _ = std::fs::remove_file(&part);
            log::error!("firmware update failed: {e}");
            progress(&UpdatePhase::Failed(e.to_string()));
        }
    }
    result
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

    fn leave(image: &[u8], places: &Places, own: &Path) -> Result<Left> {
        leave_for_recovery(image, places, own, |_| {})
    }

    #[test]
    fn our_package_is_left_with_its_recovery_written() {
        let (_dir, places, own) = phone();
        let package = ours();
        let left = leave(&eff("D80", "3_1_0", &package), &places, &own).unwrap();
        assert_eq!(left, Left { version: "3_1_0".into(), is_stock: false, recovery_written: true, package_len: package.len() as u64 });
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
        let left = leave(&eff("D80", "3_1_0", &package), &places, &own).unwrap();
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
        let (_dir, places, own) = phone();
        // An image for another model is refused before anything else is
        // checked.
        let container = [&eff::OTA_MAGIC.to_be_bytes()[..], &[0; 200]].concat();
        let other = eff::envelope::pack(&EffHeader::new("D60", "3_1_0").unwrap(), &container, [9; 8]).unwrap();
        assert!(matches!(leave(&other, &places, &own), Err(UpdateError::WrongModel { .. })));
        untouched(&places);
        // Not laid out as a signed package.
        let unsigned = zip::write(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }]).unwrap();
        assert!(leave(&eff("D80", "3_1_0", &unsigned), &places, &own).unwrap_err().to_string().contains("signed package"));
        untouched(&places);
        // No recovery image, one that is none, and one too large for the
        // partition.
        let none = signed(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }]);
        assert!(leave(&eff("D80", "3_1_0", &none), &places, &own).unwrap_err().to_string().contains("recovery.img"));
        let not_one = signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: "recovery.img", data: b"not an image", executable: false },
        ]);
        assert!(leave(&eff("D80", "3_1_0", &not_one), &places, &own).unwrap_err().to_string().contains("not a boot image"));
        let large = signed(&[
            FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true },
            FileEntry { name: "recovery.img", data: &[b'A'; PARTITION + 1], executable: false },
        ]);
        assert!(leave(&eff("D80", "3_1_0", &large), &places, &own).is_err());
        untouched(&places);
        // Not an EFF image at all.
        assert!(leave(b"PK\x03\x04 and so on", &places, &own).is_err());
        untouched(&places);
    }

    #[test]
    fn a_failed_update_takes_an_earlier_one_with_it() {
        // The package of an earlier update is replaced by the next one's,
        // so its command must not outlive it.
        let (_dir, places, own) = phone();
        leave(&eff("D80", "3_1_0", &ours()), &places, &own).unwrap();
        assert!(leave(&eff("D80", "3_1_1", b"PK\x03\x04 and nothing of a package"), &places, &own).is_err());
        let st = status(&places);
        assert_eq!((st.command, st.package_len), (None, None));
    }

    #[test]
    fn a_stock_package_is_left_with_the_stock_recovery() {
        const BOOT: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/boot.img");
        const PATCH: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/recovery-from-boot.p");
        const RESOURCES: &[u8] = include_bytes!("../../opendeskphone-recovery/testdata/recovery-resource.dat");
        let (_dir, places, own) = phone();
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
        let left = leave(&eff("D80", "1_12_14", &package), &places, &own).unwrap();
        assert_eq!(left, Left { version: "1_12_14".into(), is_stock: true, recovery_written: true, package_len: package.len() as u64 });
        // The stored package is the image minus its signature, and the
        // recovery written is the one the package's patch produces.
        assert_eq!(std::fs::read(places.nvdata.join("recovery/update.zip")).unwrap(), package);
        let want = imgdiff::stock_recovery(BOOT, PATCH, RESOURCES, PARTITION).unwrap();
        assert_eq!(std::fs::read(&places.recovery).unwrap()[..want.len()], want[..]);
        assert!(status(&places).recovery_next);

        // A stock package without its recovery's parts is refused.
        let (_dir, places, own) = phone();
        let bare = signed(&[FileEntry { name: UPDATE_BINARY, data: b"\x7fELF", executable: true }, FileEntry { name: STOCK_BOOT, data: BOOT, executable: false }]);
        assert!(leave(&eff("D80", "1_12_14", &bare), &places, &own).unwrap_err().to_string().contains(STOCK_PATCH));
        untouched(&places);
    }
}
