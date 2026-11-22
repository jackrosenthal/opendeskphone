//! End-to-end runs of the `opendeskphone-image` binary.  The vendor comparison
//! needs `test-inputs/vendor` (see the `test-inputs` crate).

use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_opendeskphone-image"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    )
}

fn ok(args: &[&str]) -> String {
    let (success, text) = tool(args);
    assert!(success, "opendeskphone-image {args:?} failed: {text}");
    text
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("opendeskphone-image-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file that passes for a zImage: magic at 0x24, start/end at 0x28/0x2c.
fn fake_zimage(len: usize) -> Vec<u8> {
    let mut z: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
    z[0x24..0x28].copy_from_slice(&0x016f2818u32.to_le_bytes());
    z[0x28..0x2c].copy_from_slice(&0u32.to_le_bytes());
    z[0x2c..0x30].copy_from_slice(&(len as u32).to_le_bytes());
    z
}

fn fake_dtb(len: usize) -> Vec<u8> {
    let mut d = vec![0x5a; len];
    d[..4].copy_from_slice(&[0xd0, 0x0d, 0xfe, 0xed]);
    d[4..8].copy_from_slice(&(len as u32).to_be_bytes());
    d
}

#[test]
fn build_a_whole_image() {
    let s = Scratch::new("whole");
    std::fs::write(s.path("zImage"), fake_zimage(100_001)).unwrap();
    std::fs::write(s.path("board.dtb"), fake_dtb(9_001)).unwrap();
    ok(&[
        "kernel-image",
        "--zimage",
        &s.path("zImage"),
        "--dtb",
        &s.path("board.dtb"),
        "--model",
        "d60",
        "--out",
        &s.path("k.bin"),
    ]);
    let k = std::fs::read(s.path("k.bin")).unwrap();
    assert_eq!(k.len(), 109_004); // padded to a word
    assert_eq!(&k[0x28..0x30], &[0, 0, 0, 2, 0xcc, 0xa9, 1, 2]); // 0x02000000, 0x0201a9cc

    let root = s.0.join("root");
    std::fs::create_dir_all(root.join("sbin")).unwrap();
    std::fs::create_dir_all(root.join("lib/modules")).unwrap();
    std::fs::write(root.join("sbin/dphone"), vec![1u8; 50_000]).unwrap();
    std::os::unix::fs::symlink("dphone", root.join("sbin/init")).unwrap();

    let text = ok(&[
        "image",
        "build",
        "--kernel",
        &s.path("k.bin"),
        "--rootfs",
        root.to_str().unwrap(),
        "--model",
        "D60",
        "--out",
        &s.path("fw.eff"),
        "--ota-out",
        &s.path("fw.ota"),
        "--mtime",
        "1792415108",
        "--node",
        "dev/console:c:5:1:600",
    ]);
    assert!(text.contains("D60 3_0_0"), "{text}");
    let info = ok(&["eff", "info", &s.path("fw.eff")]);
    assert!(info.contains("model D60 version 3_0_0"), "{info}");
    assert!(info.contains("\"3_0_0_D60_firmware.bin\""), "{info}");
    assert!(info.contains("\"Mon Oct 19 13:05:08 2026\""), "{info}");
    assert!(info.contains("kernel 109004 bytes"), "{info}");

    // Reproducible, and the pieces come back out unchanged.
    ok(&[
        "image",
        "build",
        "--kernel",
        &s.path("k.bin"),
        "--rootfs",
        root.to_str().unwrap(),
        "--model",
        "d60",
        "--out",
        &s.path("fw2.eff"),
        "--mtime",
        "1792415108",
        "--node",
        "dev/console:c:5:1:600",
    ]);
    assert_eq!(
        std::fs::read(s.path("fw.eff")).unwrap(),
        std::fs::read(s.path("fw2.eff")).unwrap()
    );
    ok(&["eff", "unpack", &s.path("fw.eff"), &s.path("back.ota")]);
    assert_eq!(
        std::fs::read(s.path("back.ota")).unwrap(),
        std::fs::read(s.path("fw.ota")).unwrap()
    );
    ok(&["ota", "unpack", &s.path("back.ota"), &s.path("parts")]);
    assert_eq!(std::fs::read(s.0.join("parts/kernel.bin")).unwrap(), k);
    let listing = ok(&[
        "jffs2",
        "list",
        s.0.join("parts/rootfs.jffs2").to_str().unwrap(),
    ]);
    assert!(listing.contains("sbin/init -> dphone"), "{listing}");
    assert!(listing.contains("5,1 dev/console"), "{listing}");
    assert!(
        listing.contains("040755") && listing.contains("lib/modules"),
        "{listing}"
    );
    ok(&[
        "jffs2",
        "extract",
        s.0.join("parts/rootfs.jffs2").to_str().unwrap(),
        &s.path("out"),
    ]);
    assert_eq!(
        std::fs::read(s.0.join("out/sbin/dphone")).unwrap(),
        vec![1u8; 50_000]
    );

    // An EFF for another model carries that model inside.
    ok(&[
        "eff",
        "pack",
        &s.path("fw.ota"),
        &s.path("d40.eff"),
        "--model",
        "d40",
        "--version",
        "3_0_0",
    ]);
    assert!(ok(&["eff", "info", &s.path("d40.eff")]).contains("model D40"));
}

#[test]
fn d80_images_carry_a_zip() {
    let s = Scratch::new("d80");
    // A zip ending the way a signed package does: an end record with no
    // comment, followed by the six-byte signature footer.
    let mut zip = b"PK\x03\x04".to_vec();
    zip.extend((0..10_000).map(|i| (i * 13) as u8));
    zip.extend(b"PK\x05\x06");
    zip.extend([0u8; 18]);
    zip.extend([6, 0, 0xff, 0xff, 6, 0]);
    std::fs::write(s.path("update.zip"), &zip).unwrap();
    let key = concat!(env!("CARGO_MANIFEST_DIR"), "/keys/d80-ota.key.pem");
    // The default key is the one in the tree, so the image is signed
    // without --key, and an explicit --key produces the same bytes.
    ok(&["eff", "pack", &s.path("update.zip"), &s.path("d80.eff"), "--model", "d80", "--version", "1_12_14"]);
    ok(&["eff", "pack", &s.path("update.zip"), &s.path("d80b.eff"), "--model", "D80", "--version", "1_12_14", "--key", key]);
    assert_eq!(
        std::fs::read(s.path("d80.eff")).unwrap(),
        std::fs::read(s.path("d80b.eff")).unwrap()
    );
    // Reproducible: with a fixed salt, nothing in the output is random.
    ok(&[
        "eff",
        "pack",
        &s.path("update.zip"),
        &s.path("d80c.eff"),
        "--model",
        "D80",
        "--version",
        "1_12_14",
        "--salt",
        "0102030405060708",
    ]);
    ok(&[
        "eff",
        "pack",
        &s.path("update.zip"),
        &s.path("d80d.eff"),
        "--model",
        "D80",
        "--version",
        "1_12_14",
        "--salt",
        "0102030405060708",
    ]);
    assert_eq!(
        std::fs::read(s.path("d80c.eff")).unwrap(),
        std::fs::read(s.path("d80d.eff")).unwrap()
    );
    let info = ok(&["eff", "info", &s.path("d80.eff")]);
    assert!(info.contains("model D80 version 1_12_14"), "{info}");
    assert!(
        info.contains(&format!("Android OTA package (zip): {} bytes", zip.len())),
        "{info}"
    );
    ok(&["eff", "unpack", &s.path("d80.eff"), &s.path("back.zip")]);
    assert_eq!(std::fs::read(s.path("back.zip")).unwrap(), zip);
    // An OTA container is not a D80 payload.
    let mut ota = 0x7d9f_8001u32.to_be_bytes().to_vec();
    ota.extend(&zip[4..]);
    std::fs::write(s.path("x.ota"), &ota).unwrap();
    let (success, text) = tool(&["eff", "pack", &s.path("x.ota"), &s.path("x.eff"), "--model", "D80", "--version", "1_12_14"]);
    assert!(!success && text.contains("Android OTA package"), "{text}");
}

#[test]
fn refusals() {
    let s = Scratch::new("refuse");
    std::fs::write(s.path("big"), fake_zimage(0x200000)).unwrap();
    std::fs::write(s.path("small"), fake_zimage(4096)).unwrap();
    std::fs::write(s.path("dtb"), fake_dtb(4096)).unwrap();
    // Too big for the slot.
    let (success, text) = tool(&[
        "kernel-image",
        "--zimage",
        &s.path("big"),
        "--dtb",
        &s.path("dtb"),
        "--model",
        "d60",
        "--out",
        &s.path("k"),
    ]);
    assert!(!success && text.contains("Flash read failed"), "{text}");
    // Unknown model, not a zImage, not a DTB.
    assert!(
        !tool(&[
            "kernel-image",
            "--zimage",
            &s.path("small"),
            "--dtb",
            &s.path("dtb"),
            "--model",
            "d99",
            "--out",
            &s.path("k")
        ])
        .0
    );
    assert!(
        !tool(&[
            "kernel-image",
            "--zimage",
            &s.path("dtb"),
            "--dtb",
            &s.path("dtb"),
            "--model",
            "d60",
            "--out",
            &s.path("k")
        ])
        .0
    );
    assert!(
        !tool(&[
            "kernel-image",
            "--zimage",
            &s.path("small"),
            "--dtb",
            &s.path("small"),
            "--model",
            "d60",
            "--out",
            &s.path("k")
        ])
        .0
    );
    // image build: unpatched kernel (header length != file length after appending), and a rootfs without /sbin/init.
    ok(&[
        "kernel-image",
        "--zimage",
        &s.path("small"),
        "--dtb",
        &s.path("dtb"),
        "--model",
        "d60",
        "--out",
        &s.path("k"),
    ]);
    let mut raw = fake_zimage(4096);
    raw.extend(fake_dtb(4096));
    std::fs::write(s.path("unpatched"), raw).unwrap();
    std::fs::create_dir_all(s.0.join("root/etc")).unwrap();
    let root = s.path("root");
    let (success, text) = tool(&[
        "image",
        "build",
        "--kernel",
        &s.path("unpatched"),
        "--rootfs",
        &root,
        "--model",
        "d60",
        "--out",
        &s.path("x.eff"),
    ]);
    assert!(!success && text.contains("kernel-image"), "{text}");
    let (success, text) = tool(&[
        "image",
        "build",
        "--kernel",
        &s.path("k"),
        "--rootfs",
        &root,
        "--model",
        "d60",
        "--out",
        &s.path("x.eff"),
    ]);
    assert!(!success && text.contains("/sbin/init"), "{text}");
    assert!(!Path::new(&s.path("x.eff")).exists());
}

#[test]
fn vendor_references() {
    let Some(dir) = test_inputs::input("vendor") else { return };
    let s = Scratch::new("vendor");
    let p = |rel: &str| dir.join(rel).to_str().unwrap().to_owned();
    // Vendor EFF -> OTA -> parts -> OTA -> EFF, byte for byte.
    let vendor = p("D60/2_9_28_D60_firmware.eff");
    ok(&["eff", "unpack", &vendor, &s.path("v.ota")]);
    ok(&["ota", "unpack", &s.path("v.ota"), &s.path("parts")]);
    ok(&[
        "ota",
        "pack",
        "--kernel",
        &s.path("parts/kernel.bin"),
        "--rootfs",
        &s.path("parts/rootfs.jffs2"),
        "--date",
        "Tue Oct 27 13:05:08 2026",
        "--name",
        "2_9_28_D60_firmware.bin",
        &s.path("re.ota"),
    ]);
    ok(&[
        "eff",
        "pack",
        &s.path("re.ota"),
        &s.path("re.eff"),
        "--model",
        "D60",
        "--version",
        "2_9_28",
        "--salt",
        "350421cf0597008e",
    ]);
    assert!(std::fs::read(s.path("re.eff")).unwrap() == std::fs::read(&vendor).unwrap());
}
