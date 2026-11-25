//! What a firmware image for the D80 is made of.
//!
//! The D80 is an Android device underneath: its U-Boot starts an Android
//! boot image from the first partition of the eMMC, and its recovery
//! installs an Android OTA package (a zip file).  Three things are made
//! here:
//!
//! - [`BootImage`]: the Android boot image (header version 0) with the
//!   kernel and a ramdisk.
//! - [`package`]: the OTA package, which writes the boot image to
//!   `mmcblk0p1` and the root file system image to `mmcblk0p5`, signed
//!   ([`sign`]) with the key in `keys/`, which is not a secret.  It also
//!   carries the recovery image, a boot image whose ramdisk is the
//!   recovery (`crates/opendeskphone-recovery`): `dphone` writes that to
//!   `mmcblk0p2` before it has the recovery install the package.
//! - [`zl38040_firmware`]: the firmware of the ZL38040 voice processor,
//!   which is nowhere but inside the stock kernel, in the form the
//!   mainline driver loads.
//!
//! The partitions, as the stock recovery's `fstab` names them (`p3` is the
//! extended partition that holds `p5` thru `p8`):
//!
//! | Partition | Size | Stock use | Use here |
//! | --- | --- | --- | --- |
//! | `mmcblk0p1` | 8 MiB | `/boot` | the boot image |
//! | `mmcblk0p2` | 8 MiB | `/recovery` | the recovery image, written by `dphone` |
//! | `mmcblk0p5` | 512 MiB | `/system` | the root file system (SquashFS) |
//! | `mmcblk0p6` | 512 MiB | `/cache` | nvdata (ext4, as stock made it) |
//! | `mmcblk0p7` | 8 MiB | `/device` | left alone |
//! | `mmcblk0p8` | 8 MiB | `/misc` | left alone |
//! | `mmcblk0p4` | the rest | `/data` | left alone |

use opendeskphone_recovery::sig::sign;
use opendeskphone_recovery::zip::{FileEntry as ZipEntry, write as zip};
use sha1::{Digest, Sha1};

/// Size of `mmcblk0p1`, which holds the boot image: 16384 sectors.
pub const BOOT_PARTITION: usize = 16384 * 512;
/// Size of `mmcblk0p2`, which holds the recovery image: as large as
/// `mmcblk0p1`.
pub const RECOVERY_PARTITION: usize = BOOT_PARTITION;
/// Size of `mmcblk0p5`, which holds the root file system: 1048575 sectors.
pub const ROOT_PARTITION: usize = 1_048_575 * 512;

/// Where the stock boot image has U-Boot load the kernel.  The kernel finds
/// the start of RAM from it (the top five bits), so it must stay in the
/// first 128 MiB.
pub const KERNEL_ADDR: u32 = 0x1080_8000;
/// Where the stock boot image has U-Boot load the ramdisk.
pub const RAMDISK_ADDR: u32 = 0x1180_0000;
/// The stock boot image's address for a second-stage image; it has none.
pub const SECOND_ADDR: u32 = 0x1170_0000;
/// The stock boot image's address for the boot tags (U-Boot ignores it).
pub const TAGS_ADDR: u32 = 0x1080_0100;
/// The stock boot image's page size.
pub const PAGE: usize = 2048;

const BOOT_MAGIC: &[u8; 8] = b"ANDROID!";
const NAME_LEN: usize = 16;
const CMDLINE_LEN: usize = 512;
const ID_OFFSET: usize = 48 + NAME_LEN + CMDLINE_LEN;
/// The header: magic, ten words, name, command line, id, and the rest of
/// the command line.
const HEADER_LEN: usize = ID_OFFSET + 32 + 1024;

const ZIMAGE_MAGIC: u32 = 0x016f_2818;
const DTB_MAGIC: [u8; 4] = [0xd0, 0x0d, 0xfe, 0xed];

fn word(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn pad(len: usize) -> usize {
    len.div_ceil(PAGE) * PAGE
}

/// The parts of an Android boot image with a version 0 header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootImage {
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    pub kernel_addr: u32,
    pub ramdisk_addr: u32,
    pub second_addr: u32,
    pub tags_addr: u32,
    pub cmdline: String,
}

impl BootImage {
    /// A boot image with the stock image's addresses.
    pub fn new(kernel: Vec<u8>, ramdisk: Vec<u8>, cmdline: &str) -> BootImage {
        BootImage {
            kernel,
            ramdisk,
            kernel_addr: KERNEL_ADDR,
            ramdisk_addr: RAMDISK_ADDR,
            second_addr: SECOND_ADDR,
            tags_addr: TAGS_ADDR,
            cmdline: cmdline.to_string(),
        }
    }

    /// The image's id: the SHA-1 of the kernel, its length, the ramdisk,
    /// its length, and the length of the second-stage image (none).
    fn id(&self) -> [u8; 20] {
        let mut h = Sha1::new();
        for part in [&self.kernel[..], &self.ramdisk[..], &[]] {
            h.update(part);
            h.update((part.len() as u32).to_le_bytes());
        }
        h.finalize().into()
    }

    /// Reasons the D80 would not start this image.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        let k = &self.kernel;
        if k.len() < 0x30 || word(k, 0x24) != ZIMAGE_MAGIC {
            p.push("the kernel is not an ARM zImage".to_string());
        } else {
            // U-Boot hands the kernel boot tags and no devicetree, so the
            // devicetree must follow the zImage, at the length its header
            // gives.
            let end = (word(k, 0x2c) - word(k, 0x28)) as usize;
            if k.get(end..end + 4) != Some(&DTB_MAGIC[..]) {
                p.push("the kernel has no devicetree after the zImage (U-Boot passes none)".to_string());
            }
        }
        if self.ramdisk.is_empty() {
            p.push("there is no ramdisk (the stock U-Boot has not been seen to start an image without one)".to_string());
        }
        if self.cmdline.len() >= CMDLINE_LEN {
            p.push(format!("the command line is {} bytes; the header holds {}", self.cmdline.len(), CMDLINE_LEN - 1));
        }
        let total = self.len();
        if total > BOOT_PARTITION {
            p.push(format!("the image is {total} bytes; the boot partition holds {BOOT_PARTITION}"));
        }
        // U-Boot loads the ramdisk without regard for the kernel before it.
        let kernel_end = self.kernel_addr as usize + k.len();
        if self.ramdisk_addr > self.kernel_addr && kernel_end > self.ramdisk_addr as usize {
            p.push(format!("the kernel ends at {kernel_end:#x}, past the ramdisk at {:#x}", self.ramdisk_addr));
        }
        p
    }

    /// Length of the image as written.
    pub fn len(&self) -> usize {
        PAGE + pad(self.kernel.len()) + pad(self.ramdisk.len())
    }

    /// The image as written to the boot partition.
    pub fn build(&self) -> Result<Vec<u8>, String> {
        let problems = self.problems();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let mut out = vec![0u8; self.len()];
        out[..8].copy_from_slice(BOOT_MAGIC);
        let words = [
            self.kernel.len() as u32,
            self.kernel_addr,
            self.ramdisk.len() as u32,
            self.ramdisk_addr,
            0,
            self.second_addr,
            self.tags_addr,
            PAGE as u32,
        ];
        for (i, w) in words.iter().enumerate() {
            out[8 + 4 * i..12 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        let cmdline = 48 + NAME_LEN;
        out[cmdline..cmdline + self.cmdline.len()].copy_from_slice(self.cmdline.as_bytes());
        out[ID_OFFSET..ID_OFFSET + 20].copy_from_slice(&self.id());
        out[PAGE..PAGE + self.kernel.len()].copy_from_slice(&self.kernel);
        let ramdisk = PAGE + pad(self.kernel.len());
        out[ramdisk..ramdisk + self.ramdisk.len()].copy_from_slice(&self.ramdisk);
        Ok(out)
    }

    /// Take a boot image apart.  Data after the image (a partition's
    /// remainder) is ignored.
    pub fn parse(data: &[u8]) -> Result<BootImage, String> {
        if data.len() < HEADER_LEN || &data[..8] != BOOT_MAGIC {
            return Err("not an Android boot image".into());
        }
        if word(data, 36) as usize != PAGE || word(data, 40) != 0 {
            return Err(format!(
                "not a boot image as the D80 has them (page size {}, header version {})",
                word(data, 36),
                word(data, 40)
            ));
        }
        let (kernel_len, ramdisk_len) = (word(data, 8) as usize, word(data, 16) as usize);
        let ramdisk = PAGE + pad(kernel_len);
        let part = |off: usize, len: usize| {
            data.get(off..off + len).map(<[u8]>::to_vec).ok_or_else(|| "the boot image is cut short".to_string())
        };
        let cmdline = &data[48 + NAME_LEN..ID_OFFSET];
        let cmdline = &cmdline[..cmdline.iter().position(|b| *b == 0).unwrap_or(cmdline.len())];
        let image = BootImage {
            kernel: part(PAGE, kernel_len)?,
            ramdisk: part(ramdisk, ramdisk_len)?,
            kernel_addr: word(data, 12),
            ramdisk_addr: word(data, 20),
            second_addr: word(data, 28),
            tags_addr: word(data, 32),
            cmdline: String::from_utf8_lossy(cmdline).into_owned(),
        };
        if image.id() != data[ID_OFFSET..ID_OFFSET + 20] {
            return Err("the boot image's id is not the SHA-1 of its parts".into());
        }
        Ok(image)
    }
}

/// A ramdisk with nothing in it: a `newc` cpio archive of the end marker
/// alone.  The kernel unpacks it, finds no `/init`, and mounts the root
/// file system that its command line names.
pub fn empty_ramdisk() -> Vec<u8> {
    const NAME: &[u8] = b"TRAILER!!!\0";
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(b"070701");
    // inode, mode, uid, gid, links (1), mtime, size, four device numbers,
    // name length, checksum
    for field in [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, NAME.len(), 0] {
        out.extend_from_slice(format!("{field:08X}").as_bytes());
    }
    out.extend_from_slice(NAME);
    out.resize(512, 0);
    out
}

/// The stock firmware's name for the D80 (`ro.product.device`), which the
/// package checks before it writes anything.
pub const DEVICE: &str = opendeskphone_recovery::ANDROID_DEVICE;

/// The script the package's `update-binary` runs: the root file system,
/// then the kernel, each written whole to its partition.  The kernel is
/// last, as in the stock package, so that a package cut short leaves the
/// kernel that was there.
pub fn updater_script() -> String {
    format!(
        "getprop(\"ro.product.device\") == \"{DEVICE}\" || abort(\"This package is for \\\"{DEVICE}\\\" devices; this is a \\\"\" + getprop(\"ro.product.device\") + \"\\\".\");\n\
         ui_print(\"Open Desk Phone\");\n\
         show_progress(0.800000, 0);\n\
         package_extract_file(\"rootfs.img\", \"/dev/block/mmcblk0p5\");\n\
         show_progress(0.200000, 0);\n\
         package_extract_file(\"boot.img\", \"/dev/block/mmcblk0p1\");\n\
         set_progress(1.000000);\n"
    )
}

/// The OTA package: the boot image, the root file system image, the script
/// that writes them, and the stock `update-binary` that runs the script
/// (a recovery runs whatever a package brings under that name).  The
/// package also carries the recovery image, which its script does not
/// write: a recovery does not replace itself.  `dphone` writes it, before
/// it restarts the phone into it.  The metadata, the package's first file,
/// carries the package's checksum, which provisioning reads from the start
/// of a download.  The package is signed with `key`, and carries `cert`;
/// see [`sign`].
pub fn package(boot: &[u8], recovery: &[u8], rootfs: &[u8], update_binary: &[u8], version: &str, key: &str, cert: &str) -> Result<Vec<u8>, String> {
    BootImage::parse(boot)?;
    let ramdisk = BootImage::parse(recovery).map_err(|e| format!("the recovery image: {e}"))?.ramdisk;
    if ramdisk == empty_ramdisk() {
        return Err("the recovery image has no ramdisk: it would start the system, not a recovery".into());
    }
    if recovery.len() > RECOVERY_PARTITION {
        return Err(format!("the recovery image is {} bytes; its partition holds {RECOVERY_PARTITION}", recovery.len()));
    }
    if boot.len() > BOOT_PARTITION {
        return Err(format!("the boot image is {} bytes; its partition holds {BOOT_PARTITION}", boot.len()));
    }
    if rootfs.len() > ROOT_PARTITION {
        return Err(format!("the root file system image is {} bytes; its partition holds {ROOT_PARTITION}", rootfs.len()));
    }
    if rootfs.get(..4) != Some(b"hsqs") {
        return Err("the root file system image is not a SquashFS image".into());
    }
    if update_binary.get(..4) != Some(b"\x7fELF") {
        return Err("update-binary is not a program".into());
    }
    let checksum = opendeskphone_recovery::checksum(!eff::crc::crc(boot), boot.len() as u64, !eff::crc::crc(rootfs), rootfs.len() as u64);
    let metadata = format!(
        "post-build=OpenDeskPhone/{DEVICE}/{version}\npre-device={DEVICE}\n{}={checksum}\n",
        opendeskphone_recovery::CHECKSUM_KEY
    );
    let script = updater_script();
    let unsigned = zip(&[
        ZipEntry { name: opendeskphone_recovery::METADATA_ENTRY, data: metadata.as_bytes(), executable: false },
        ZipEntry { name: "META-INF/com/google/android/update-binary", data: update_binary, executable: true },
        ZipEntry { name: "META-INF/com/google/android/updater-script", data: script.as_bytes(), executable: false },
        ZipEntry { name: opendeskphone_recovery::ROOTFS_ENTRY, data: rootfs, executable: false },
        ZipEntry { name: opendeskphone_recovery::BOOT_ENTRY, data: boot, executable: false },
        ZipEntry { name: opendeskphone_recovery::RECOVERY_ENTRY, data: recovery, executable: false },
    ])?;
    sign(&unsigned, key, cert)
}

/// The largest kernel a stock zImage is taken to unpack to.
const KERNEL_MAX: usize = 64 << 20;

/// The stock kernel as it runs: the zImage's compressed part, unpacked.
fn unpacked_kernel(zimage: &[u8]) -> Result<Vec<u8>, String> {
    // A gzip member with the deflate method and no optional fields, which
    // is how the kernel build compresses.  The bytes can also occur by
    // chance in the decompressor before it, so every place is tried.
    const GZIP: [u8; 4] = [0x1f, 0x8b, 0x08, 0x00];
    for at in zimage.windows(4).enumerate().filter(|(_, w)| *w == GZIP).map(|(i, _)| i) {
        let Some(body) = zimage.get(at + 10..) else { continue };
        if let Ok(out) = miniz_oxide::inflate::decompress_to_vec_with_limit(body, KERNEL_MAX)
            && out.len() > 1 << 20
        {
            return Ok(out);
        }
    }
    Err("no gzip-compressed kernel in the zImage".into())
}

fn hex_byte(pair: &[u8]) -> Option<u8> {
    u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()
}

/// One record of the kernel's binary firmware form (`struct ihex_binrec`):
/// the address and the length, big-endian, then the data, padded to a
/// multiple of four bytes.
fn binrec(out: &mut Vec<u8>, addr: u32, data: &[u8]) {
    out.extend_from_slice(&addr.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// The ZL38040's firmware, from a stock boot image, as the mainline driver
/// loads it (`zl38040.fw`).
///
/// The stock kernel has the firmware built in, as the text of Motorola
/// S-records (`audio/ZLS38040App_v2.2.2.s3`).  The driver takes the
/// kernel's binary form of Intel HEX records, with the address execution
/// starts at as a record of its own: what `ihex2fw -j` makes of the same
/// records.
pub fn zl38040_firmware(boot_image: &[u8]) -> Result<Vec<u8>, String> {
    let kernel = unpacked_kernel(&BootImage::parse(boot_image)?.kernel)?;
    // The text starts with its header record and has an S3 record on the
    // next line.
    let is_start = |i: usize| {
        kernel[i..].starts_with(b"S0")
            && kernel[i..].iter().position(|b| *b == b'\r').is_some_and(|n| n < 80 && kernel[i + n..].starts_with(b"\r\nS3"))
    };
    let start = (0..kernel.len().saturating_sub(4)).find(|i| is_start(*i)).ok_or("no S-records in the stock kernel")?;
    let mut out = Vec::new();
    let mut entry = None;
    for (n, line) in kernel[start..].split(|b| *b == b'\n').enumerate() {
        let bad = |what: &str| format!("S-record {}: {what}", n + 1);
        let line = line.strip_suffix(b"\r").ok_or_else(|| bad("no line end"))?;
        if line.len() < 4 || line[0] != b'S' || !line.len().is_multiple_of(2) {
            return Err(bad("not a record"));
        }
        let raw: Vec<u8> = line[2..].chunks(2).map(hex_byte).collect::<Option<_>>().ok_or_else(|| bad("not hexadecimal"))?;
        if raw[0] as usize != raw.len() - 1 || raw.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0xff {
            return Err(bad("wrong length or checksum"));
        }
        match line[1] {
            b'0' => {}
            b'3' | b'7' if raw.len() < 6 => return Err(bad("too short")),
            b'3' => {
                let addr = u32::from_be_bytes(raw[1..5].try_into().unwrap());
                let data = &raw[5..raw.len() - 1];
                // The driver writes a record within one 256-byte page of
                // the processor, in 16-bit words.
                if data.is_empty() || !data.len().is_multiple_of(2) || (addr & 0xff) as usize + data.len() > 0x100 {
                    return Err(bad("not whole words within one page"));
                }
                binrec(&mut out, addr, data);
            }
            b'7' => {
                // The address execution starts at; it ends the text.
                // `ihex2fw` stores it in the byte order of the machine
                // it runs on, which for every stock firmware file made
                // so is the low byte first, and so the driver reads it.
                entry = Some(raw[1..5].iter().rev().copied().collect::<Vec<u8>>());
                break;
            }
            _ => return Err(bad("of a kind the firmware does not have")),
        }
    }
    let entry = entry.ok_or("the S-records have no start address")?;
    if out.is_empty() {
        return Err("the S-records have no data".into());
    }
    binrec(&mut out, 0, &entry);
    // The end: a record of no length.
    out.extend_from_slice(&[0; 6]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zImage header and a devicetree's first word after it.
    fn kernel(len: usize) -> Vec<u8> {
        let mut k = vec![0u8; len];
        k[0x24..0x28].copy_from_slice(&ZIMAGE_MAGIC.to_le_bytes());
        k[0x2c..0x30].copy_from_slice(&(len as u32 - 64).to_le_bytes());
        k[len - 64..len - 60].copy_from_slice(&DTB_MAGIC);
        k
    }

    #[test]
    fn a_boot_image_reads_back() {
        let image = BootImage::new(kernel(5000), empty_ramdisk(), "console=ttymxc0,115200");
        let data = image.build().unwrap();
        assert_eq!(data.len(), PAGE + 3 * PAGE + PAGE);
        assert_eq!(&data[..8], b"ANDROID!");
        assert_eq!(word(&data, 12), 0x1080_8000);
        assert_eq!(word(&data, 20), 0x1180_0000);
        assert_eq!(word(&data, 36), 2048);
        assert_eq!(BootImage::parse(&data).unwrap(), image);
        // In a partition, with what an earlier image left after it.
        let mut partition = data.clone();
        partition.resize(BOOT_PARTITION, 0xa5);
        assert_eq!(BootImage::parse(&partition).unwrap(), image);
        // A changed byte of the kernel no longer matches the id.
        let mut bad = data;
        bad[PAGE + 100] ^= 1;
        assert!(BootImage::parse(&bad).unwrap_err().contains("id"));
    }

    #[test]
    fn what_the_d80_would_not_start_is_refused() {
        let no_dtb = BootImage::new(vec![0; 5000], empty_ramdisk(), "");
        assert!(no_dtb.build().unwrap_err().contains("zImage"));
        let mut k = kernel(5000);
        k[5000 - 64] = 0;
        assert!(BootImage::new(k, empty_ramdisk(), "").build().unwrap_err().contains("devicetree"));
        assert!(BootImage::new(kernel(5000), Vec::new(), "").build().unwrap_err().contains("ramdisk"));
        let large = BootImage::new(kernel(BOOT_PARTITION), empty_ramdisk(), "");
        assert!(large.build().unwrap_err().contains("boot partition"));
    }

    #[test]
    fn the_empty_ramdisk_is_one_end_marker() {
        let r = empty_ramdisk();
        assert_eq!(r.len(), 512);
        assert_eq!(&r[..6], b"070701");
        // The name's length, with its NUL, in the twelfth field.
        assert_eq!(&r[6 + 11 * 8..6 + 12 * 8], b"0000000B");
        assert_eq!(&r[110..121], b"TRAILER!!!\0");
        assert!(r[121..].iter().all(|b| *b == 0));
    }

    #[test]
    fn a_package_has_what_recovery_runs() {
        let boot = BootImage::new(kernel(5000), empty_ramdisk(), "").build().unwrap();
        let rootfs = b"hsqs-and-the-rest".to_vec();
        let recovery = BootImage::new(kernel(5000), b"a ramdisk".to_vec(), "").build().unwrap();
        let z = package(&boot, &recovery, &rootfs, b"\x7fELF...", "3_1_0", KEY, CERT).unwrap();
        let text = String::from_utf8_lossy(&z);
        for name in [
            "META-INF/com/google/android/update-binary",
            "META-INF/com/google/android/updater-script",
            "rootfs.img",
            "boot.img",
            "recovery.img",
        ] {
            assert!(text.contains(name), "{name}");
        }
        assert!(text.contains("package_extract_file(\"boot.img\", \"/dev/block/mmcblk0p1\");"));
        assert!(text.contains("package_extract_file(\"rootfs.img\", \"/dev/block/mmcblk0p5\");"));
        assert!(text.contains("pre-device=model80"));
        // The script writes the system, and leaves the recovery alone.
        assert!(!text.contains("mmcblk0p2"));
        // The package ends the way a signed package does, and the pieces
        // a recovery needs are inside.
        assert!(opendeskphone_recovery::sig::has_footer(&mut std::io::Cursor::new(&z), z.len() as u64));
        let mut archive = opendeskphone_recovery::zip::Archive::open(std::io::Cursor::new(&z), z.len() as u64).unwrap();
        assert_eq!(archive.read("recovery.img", RECOVERY_PARTITION).unwrap(), recovery);
        // The checksum in the metadata is the one the recovery records from
        // the archive's directory once installed.
        let metadata = archive.read(opendeskphone_recovery::METADATA_ENTRY, 4096).unwrap();
        let checksum = opendeskphone_recovery::package_checksum(&archive).unwrap();
        assert!(String::from_utf8(metadata).unwrap().contains(&format!("\nopendeskphone-checksum={checksum}\n")));
        assert!(package(&boot, &recovery, b"not squashfs", b"\x7fELF", "3_1_0", KEY, CERT).is_err());
        assert!(package(b"not a boot image", &recovery, &rootfs, b"\x7fELF", "3_1_0", KEY, CERT).is_err());
        assert!(package(&boot, &recovery, &rootfs, b"#!/bin/sh", "3_1_0", KEY, CERT).is_err());
        // A boot image with the empty ramdisk is no recovery.
        assert!(package(&boot, &boot, &rootfs, b"\x7fELF", "3_1_0", KEY, CERT).unwrap_err().contains("ramdisk"));
    }

    const KEY: &str = include_str!("../keys/d80-ota.key.pem");
    const CERT: &str = include_str!("../keys/d80-ota.x509.pem");

    /// The firmware of a made-up stock kernel: two data records and the
    /// start address, as `ihex2fw -j` writes them.
    #[test]
    fn firmware_comes_out_of_a_stock_kernel() {
        fn srec(kind: u8, addr: u32, data: &[u8]) -> String {
            let mut raw = vec![(data.len() + 5) as u8];
            raw.extend_from_slice(&addr.to_be_bytes());
            raw.extend_from_slice(data);
            let sum = raw.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            raw.push(!sum);
            let hex: String = raw.iter().map(|b| format!("{b:02X}")).collect();
            format!("S{}{hex}\r\n", kind as char)
        }
        let mut text = String::from("S00600004844521B\r\n");
        text += &srec(b'3', 0x0008_0128, &[1, 2, 3, 4]);
        text += &srec(b'3', 0x000b_dbce, &[5, 6]);
        text += &srec(b'7', 0x0009_e140, &[]);
        let mut vmlinux = vec![0x5au8; 2 << 20];
        vmlinux[0x1234..0x1234 + text.len()].copy_from_slice(text.as_bytes());
        // A zImage: a gzip header and the kernel deflated, then a devicetree.
        let mut zimage = vec![0u8; 0x100];
        zimage.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 2, 3]);
        zimage.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&vmlinux, 6));
        let end = zimage.len();
        zimage[0x24..0x28].copy_from_slice(&ZIMAGE_MAGIC.to_le_bytes());
        zimage[0x2c..0x30].copy_from_slice(&(end as u32).to_le_bytes());
        zimage.extend_from_slice(&DTB_MAGIC);
        let boot = BootImage::new(zimage, empty_ramdisk(), "").build().unwrap();
        let fw = zl38040_firmware(&boot).unwrap();
        let want: Vec<u8> = [
            &[0x00, 0x08, 0x01, 0x28, 0x00, 0x04, 1, 2, 3, 4, 0, 0][..],
            &[0x00, 0x0b, 0xdb, 0xce, 0x00, 0x02, 5, 6][..],
            &[0, 0, 0, 0, 0x00, 0x04, 0x40, 0xe1, 0x09, 0x00, 0, 0][..],
            &[0; 6][..],
        ]
        .concat();
        assert_eq!(fw, want);
    }
}
