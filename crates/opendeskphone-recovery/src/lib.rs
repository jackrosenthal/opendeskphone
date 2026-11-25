//! The D80's recovery, and what the rest of the firmware shares with it.
//!
//! A D80 has one kernel and one root file system, so a running system
//! cannot replace itself.  As on the stock firmware, a second small system
//! does it: the recovery, an Android boot image in the second partition of
//! the eMMC, which the boot loader starts in place of the first when its
//! `bootcmd` says so.  An update goes like this:
//!
//! 1. `dphone` (`opendeskphone-update`) puts the OTA package in nvdata,
//!    which is the stock `/cache`, as `recovery/update.zip`, writes the
//!    recovery image that the package brings to the second partition,
//!    names the package in `recovery/command`, and sets `bootcmd` to start
//!    the recovery ([`command`], [`env`]).
//! 2. The phone restarts into the recovery (the program `recovery` of this
//!    crate, built with the feature `program`).  It reads the command,
//!    checks the package's signature against its keys ([`sig`]), runs the
//!    package's `update-binary`, sets `bootcmd` back, and restarts.
//!
//! That is what the stock firmware and the stock recovery do with each
//! other, with the same files in the same places, so either recovery
//! installs a package that the other system left for it: `dphone` writes
//! the stock recovery, which it makes from a stock package ([`imgdiff`]),
//! before it leaves a stock package to install.

pub mod command;
pub mod der;
pub mod env;
pub mod imgdiff;
pub mod install;
pub mod props;
pub mod sig;
pub mod zip;

/// The eMMC, in whose first megabyte the boot loader keeps its environment.
pub const DISK: &str = "/dev/mmcblk0";
/// The partition of the recovery image.
pub const RECOVERY_PARTITION: &str = "/dev/mmcblk0p2";
/// The stock `/cache`, which is `dphone`'s nvdata.
pub const CACHE_PARTITION: &str = "/dev/mmcblk0p6";
/// The name of the recovery image in one of our packages.  The package's
/// script does not write it: `dphone` does, before the recovery runs.
pub const RECOVERY_ENTRY: &str = "recovery.img";
/// The stock firmware's name for the D80 (`ro.product.device`), which a
/// package's script checks before it writes anything.
pub const ANDROID_DEVICE: &str = "model80";
/// A package's metadata, its first file.
pub const METADATA_ENTRY: &str = "META-INF/com/android/metadata";
/// The boot image and the root file system image in one of our packages.
pub const BOOT_ENTRY: &str = "boot.img";
pub const ROOTFS_ENTRY: &str = "rootfs.img";
/// The key in one of our packages' metadata whose value is the package's
/// [`checksum`].
pub const CHECKSUM_KEY: &str = "opendeskphone-checksum";

/// What identifies one of our packages: the CRC-32 and the size of its
/// boot image and its root file system image, as
/// `<boot CRC>-<boot size>-<root CRC>-<root size>`, the CRCs in eight
/// lowercase hexadecimal digits and the sizes in decimal bytes.  This is
/// the form of the firmware checksum that provisioning compares.
pub fn checksum(boot_crc: u32, boot_size: u64, rootfs_crc: u32, rootfs_size: u64) -> String {
    format!("{boot_crc:08x}-{boot_size}-{rootfs_crc:08x}-{rootfs_size}")
}

/// The [`checksum`] of a package, from its directory; `None` when the
/// package has no boot image or no root file system image.
pub fn package_checksum<R: std::io::Read + std::io::Seek>(archive: &zip::Archive<R>) -> Option<String> {
    let (boot, rootfs) = (archive.entry(BOOT_ENTRY)?, archive.entry(ROOTFS_ENTRY)?);
    Some(checksum(boot.crc(), boot.size, rootfs.crc(), rootfs.size))
}
