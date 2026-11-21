//! Mounting nvdata.  The partition is found by name, never by number or
//! offset (the layout comes from boot2's `mtdparts=`).
//!
//! The D80 has no flash partitions: its storage is an eMMC with the stock
//! Android partitions, and its nvdata is the one stock mounts as `/cache`
//! ([`D80_NVDATA`]), with the ext4 file system stock made there.  Stock
//! keeps nothing in it that must last (its recovery reads its orders from
//! `recovery/` there and writes its log), and stock's own settings, which
//! are in `/data`, are left as they are.

use std::io;

use log::{error, info, warn};

use crate::model::Family;
use crate::paths::NVDATA;
use crate::sys;

/// The D80's nvdata: the sixth partition of its eMMC (512 MiB, ext4),
/// which is the stock firmware's `/cache`.
pub const D80_NVDATA: &str = "/dev/mmcblk0p6";
/// The D80's root file system: the fifth partition of its eMMC, which is
/// the stock firmware's `/system`.
pub const D80_ROOT: &str = "/dev/mmcblk0p5";

/// Index of the MTD partition called `name`, from `/proc/mtd` text.
pub fn mtd_index(proc_mtd: &str, name: &str) -> Option<u32> {
    let quoted = format!("\"{name}\"");
    proc_mtd.lines().find_map(|line| {
        let (dev, rest) = line.split_once(':')?;
        if rest.trim_end().ends_with(&quoted) { dev.strip_prefix("mtd")?.parse().ok() } else { None }
    })
}

/// Outcome of the nvdata mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvdataState {
    Mounted,
    /// The real volume could not be mounted.  A tmpfs stands in so the main
    /// role can run its recovery screen and web page; nothing persists.
    /// Unlike stock, the partition is NOT erased.
    Recovery,
}

/// Mount nvdata read-write at `/nvdata`: the flash partition of that name,
/// or for the touch family (the D80) [`D80_NVDATA`].
pub fn mount(family: Option<Family>) -> NvdataState {
    let flags = libc::MS_NOATIME | libc::MS_NOSUID | libc::MS_NODEV;
    // The early-boot stage mounts nvdata to read the trial record and hands
    // it over mounted (with the same flags), which saves a second scan.
    let mounted = std::fs::read_to_string("/proc/mounts").is_ok_and(|m| is_mounted(&m));
    let result = if mounted {
        Ok("the early-boot stage".to_string())
    } else if family == Some(Family::Touch) {
        sys::mount(D80_NVDATA, NVDATA, "ext4", flags, "").map(|()| D80_NVDATA.to_string())
    } else {
        try_mount(flags)
    };
    match result {
        Ok(source) => {
            info!("nvdata mounted from {source}");
            if let Err(e) = std::fs::create_dir_all(format!("{NVDATA}/open")) {
                warn!("mkdir {NVDATA}/open: {e}");
            }
            NvdataState::Mounted
        }
        Err(e) => {
            error!("nvdata mount failed: {e}; entering recovery (partition left untouched)");
            if let Err(e) = sys::mount("tmpfs", NVDATA, "tmpfs", flags, "mode=0755,size=4m") {
                error!("recovery tmpfs on {NVDATA}: {e}");
            }
            let _ = std::fs::create_dir_all(format!("{NVDATA}/open"));
            let _ = std::fs::write(format!("{}/recovery", crate::paths::RUN_DIR), b"nvdata\n");
            NvdataState::Recovery
        }
    }
}

/// What a factory reset keeps besides stock's own list
/// (`opendeskphone_config::nvdata::FACTORY_RESET_KEEP`), relative to the nvdata
/// root.
///
/// Stock's `factory-reset.sh` keeps `etc/enet.cfg`, `reboot_reason`,
/// `phoneservice_lockdown`, the two G.729 files, and `if-setup-debug`, and
/// removes every directory but `etc`; so it removes `opendeskphone/`, and so
/// do we (host keys, authorized keys, the provisioning identity, call log, and
/// the Open Desk Phone settings file all go, as stock's own equivalents do).
/// The one addition is the trial-boot record: it is not user data, and a reset
/// that removed it would leave an unconfirmed image installed with nothing to
/// roll it back.  The DSP firmware is in the root file system and is not
/// affected.
pub const FACTORY_RESET_KEEP: &[&str] = &["opendeskphone/trial"];

/// Perform a factory reset that was requested before the last shutdown
/// (stock's marker file `factory-reset`), before anything reads nvdata.
/// Returns whether one ran.  On an error the marker stays, so the next boot
/// tries again.
pub fn run_pending_factory_reset(root: &std::path::Path) -> bool {
    let nv = opendeskphone_config::Nvdata::new(root);
    match opendeskphone_config::ConfigStore::run_pending_factory_reset(&nv, FACTORY_RESET_KEEP) {
        Ok(false) => false,
        Ok(true) => {
            sys::sync();
            warn!("factory reset performed");
            true
        }
        Err(e) => {
            error!("factory reset: {e}");
            false
        }
    }
}

/// Whether `/proc/mounts` text shows a JFFS2 volume on the nvdata mount
/// point.
fn is_mounted(proc_mounts: &str) -> bool {
    proc_mounts.lines().any(|line| {
        let mut f = line.split_ascii_whitespace();
        f.nth(1) == Some(NVDATA) && f.next() == Some("jffs2")
    })
}

fn try_mount(flags: libc::c_ulong) -> io::Result<String> {
    let proc_mtd = std::fs::read_to_string("/proc/mtd")?;
    let index = mtd_index(&proc_mtd, "nvdata")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no MTD partition named nvdata"))?;
    // JFFS2 accepts the block device or the MTD name; try both so the kernel
    // does not need MTD_BLOCK just for this.
    let candidates = [format!("/dev/mtdblock{index}"), format!("mtd{index}")];
    let mut last = io::Error::other("no candidate");
    for source in candidates {
        match sys::mount(&source, NVDATA, "jffs2", flags, "") {
            Ok(()) => return Ok(source),
            Err(e) => {
                warn!("mount {source} on {NVDATA}: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_nvdata_handed_over_mounted() {
        assert!(is_mounted("/dev/root / jffs2 ro,relatime 0 0\nmtd:nvdata /nvdata jffs2 rw,nosuid,nodev,noatime 0 0\n"));
        assert!(!is_mounted("/dev/root / jffs2 ro,relatime 0 0\nproc /proc proc rw 0 0\n"));
        assert!(!is_mounted("tmpfs /nvdata tmpfs rw 0 0\n"));
    }

    #[test]
    fn finds_partition() {
        let text = "dev:    size   erasesize  name\n\
                    mtd0: 00020000 00020000 \"boot1\"\n\
                    mtd2: 00080000 00020000 \"ctrl\"\n\
                    mtd3: 00ee0000 00020000 \"nvdata\"\n\
                    mtd8: 01000000 00020000 \"userdata\"\n";
        assert_eq!(mtd_index(text, "nvdata"), Some(3));
        assert_eq!(mtd_index(text, "ctrl"), Some(2));
        assert_eq!(mtd_index(text, "data"), None);
    }

    #[test]
    fn factory_reset_runs_once_and_keeps_the_trial_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let put = |rel: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        for f in [
            "etc/enet.cfg",
            "etc/.passwd",
            "ui-config.xml",
            "fetched-config.xml",
            "opendeskphone/settings.txtpb",
            "root/.ssh/authorized_keys",
            "opendeskphone/trial",
            "opendeskphone/ssh_host_ed25519_key",
            "opendeskphone/authorized_keys",
            "opendeskphone/calllog.txtpb",
            "ringtones/Mine.sln16",
            "certs/certs.crt",
        ] {
            put(f);
        }
        put("factory-reset");
        assert!(run_pending_factory_reset(root));
        for kept in ["etc/enet.cfg", "opendeskphone/trial"] {
            assert!(root.join(kept).exists(), "{kept} must survive");
        }
        for gone in [
            "etc/.passwd",
            "ui-config.xml",
            "fetched-config.xml",
            "opendeskphone/settings.txtpb",
            "root/.ssh/authorized_keys",
            "opendeskphone/ssh_host_ed25519_key",
            "opendeskphone/authorized_keys",
            "opendeskphone/calllog.txtpb",
            "ringtones/Mine.sln16",
            "certs/certs.crt",
            "factory-reset",
        ] {
            assert!(!root.join(gone).exists(), "{gone} must be removed");
        }
        // The marker is gone, so the next boot does not reset again.
        put("ui-config.xml");
        assert!(!run_pending_factory_reset(root));
        assert!(root.join("ui-config.xml").exists());
    }
}
