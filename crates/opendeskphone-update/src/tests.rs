//! Tests against the in-memory NAND: real `ctrl` records, replication
//! layouts, installs with bad blocks, refusals, block failures, rollback,
//! and a power cut at every flash operation.

use std::io::Read;

use opendeskphone_hal::Mtd;
use opendeskphone_state::{Model, Store, Update, UpdatePhase};
use eff::ctrl::{CtrlRecord, Side};
use eff::ota::AppHeader;
use eff::EffHeader;

use crate::bank::{self, Bank};
use crate::boot2::{self, ZIMAGE_MAGIC};
use crate::ctrl::{self, Replication};
use crate::inspect::{inspect, verify_bank};
use crate::install::{Installed, Options, Target, Updater, abandon, check_eff, select_bank};
use crate::memnand::{BLOCK, MemNand, PAGE};
use crate::trial;
use crate::{UpdateError, flash};

// ---------------------------------------------------------------- real data

/// First 304 bytes of `ctrl` from the D60 NAND dump (2026-11-08), a D40,
/// and a D50 (2026-11-12).  They hold only image names, build dates,
/// sizes, and CRCs.
const D60_DUMP: &str = "00000000297347df001df00069ddc1bc01c9c548547565204f63742032372031333a30353a3038203230323600000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32385f4436305f6669726d776172652e62696e0000000000000000000000000000000000000000000000000000000000000000000000000000000000659b259a7dabaf72001deff04107815b01c9fbfc546875204d61792020392031383a30393a3435203230323400000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32375f4436305f6669726d776172652e62696e0000000000000000000000000000000000000000000000000000000000000000000000000000000000244bfb196a81dcd5";
const D40_DUMP: &str = "00000000dec54a120015f1e0114ef10f01dcbd04467269204f63742032372031393a31313a3138203230323300000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32365f4434305f6669726d776172652e62696e00000000000000000000000000000000000000000000000000000000000000000000000000000000008d5abfdadec54a120015f1e0d206749f01dca004467269204f63742032372031393a31313a3138203230323300000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32365f4434305f6669726d776172652e62696e000000000000000000000000000000000000000000000000000000000000000000000000000000000001d1ccdc6a81dcd5";
const D50_DUMP: &str = "00000000b50b0c9c0015f1e483657f8601dcbd04467269204f63742032372031383a35313a3136203230323300000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32365f4435305f6669726d776172652e62696e000000000000000000000000000000000000000000000000000000000000000000000000000000000081b6fe39f7bebdac0015f1c82e461a3c01dbe274547565205365702032302031363a34313a3531203230323200000000000000000000000000000000000000000000000000000000000000000000000000000000325f395f32335f4435305f6669726d776172652e62696e00000000000000000000000000000000000000000000000000000000000000000000000000000000000310da1d6a81dcd5";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn raw_side(kcrc: u32, ksize: u32, fcrc: u32, fsize: u32, date: &str, name: &str, hcrc: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for w in [kcrc, ksize, fcrc, fsize] {
        v.extend_from_slice(&w.to_be_bytes());
    }
    let mut text = |s: &str| {
        let mut f = [0u8; 64];
        f[..s.len()].copy_from_slice(s.as_bytes());
        v.extend_from_slice(&f);
    };
    text(date);
    text(name);
    v.extend_from_slice(&hcrc.to_be_bytes());
    v
}

/// The record read from the live D60 on 2026-11-13 (`dd if=/dev/mtd2`),
/// after the stock updater had installed a 2_9_28 image into bank 1.
fn d60_live() -> Vec<u8> {
    let (date, name) = ("Tue Oct 27 13:05:08 2026", "2_9_28_D60_firmware.bin");
    let mut v = 1u32.to_be_bytes().to_vec();
    v.extend(raw_side(0x297347df, 0x001df000, 0x69ddc1bc, 0x01c9c548, date, name, 0x659b259a));
    v.extend(raw_side(0x297347df, 0x001df000, 0x9f464adb, 0x01c9efa4, date, name, 0x5653a296));
    v.extend_from_slice(&0x4ab36360u32.to_be_bytes());
    v
}

#[test]
fn real_ctrl_records_parse_and_round_trip() {
    for (bytes, active, names) in [
        (unhex(D60_DUMP), 0, ["2_9_28_D60_firmware.bin", "2_9_27_D60_firmware.bin"]),
        (unhex(D40_DUMP), 0, ["2_9_26_D40_firmware.bin", "2_9_26_D40_firmware.bin"]),
        (unhex(D50_DUMP), 0, ["2_9_26_D50_firmware.bin", "2_9_23_D50_firmware.bin"]),
        (d60_live(), 1, ["2_9_28_D60_firmware.bin", "2_9_28_D60_firmware.bin"]),
    ] {
        assert_eq!(bytes.len(), 304);
        let rec = CtrlRecord::parse(&bytes).expect("record CRC");
        assert_eq!(rec.active, active);
        assert_eq!(rec.boot_side(), active);
        for (side, name) in rec.sides.iter().zip(names) {
            assert_eq!(side.header().expect("header CRC").name(), name);
        }
        assert_eq!(rec.to_bytes().to_vec(), bytes, "re-encoding is byte-exact");
    }
    let d60 = CtrlRecord::parse(&unhex(D60_DUMP)).unwrap();
    let h = d60.sides[0].header().unwrap();
    assert_eq!((h.kernel_size, h.rootfs_size), (0x1df000, 0x01c9c548));
    assert_eq!(h.date(), "Tue Oct 27 13:05:08 2026");
}

#[test]
fn switching_the_dump_record_like_stock_gives_the_live_active_word_crc() {
    // The record CRC depends only on the active word (each header ends in
    // its own CRC).  Flipping the dump's active side must therefore give
    // the record CRC seen on the live phone after stock switched sides.
    let mut rec = CtrlRecord::parse(&unhex(D60_DUMP)).unwrap();
    rec.active = 1;
    assert_eq!(&rec.to_bytes()[300..], &0x4ab36360u32.to_be_bytes());
    rec.active = 0;
    assert_eq!(&rec.to_bytes()[300..], &0x6a81dcd5u32.to_be_bytes());
}

#[test]
fn erased_and_garbage_pages_are_not_records() {
    assert!(CtrlRecord::parse(&[0xff; 304]).is_err());
    assert!(CtrlRecord::parse(&[0u8; 304]).is_err());
    let mut b = unhex(D60_DUMP);
    b[3] = 1; // active flipped without fixing the CRC
    assert!(CtrlRecord::parse(&b).is_err());
    // An invalidated side (0xfe fill) leaves the record valid and the
    // other side bootable.
    let mut rec = CtrlRecord::parse(&unhex(D60_DUMP)).unwrap();
    rec.sides[1] = Side::INVALID;
    let again = CtrlRecord::parse(&rec.to_bytes()).unwrap();
    assert!(!again.sides[1].is_valid() && again.sides[0].is_valid());
    assert_eq!(again.boot_side(), 0);
    rec.active = 1;
    assert_eq!(rec.boot_side(), 0, "boot2 falls to the only valid side");
}

// ------------------------------------------------------------------ fixtures

fn prng(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

/// A fake zImage: valid boot2 header, pseudo-random body.
fn kernel(seed: u32, len: usize) -> Vec<u8> {
    let mut k = prng(seed, len);
    k[0x24..0x28].copy_from_slice(&ZIMAGE_MAGIC.to_le_bytes());
    k[0x28..0x2c].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    k[0x2c..0x30].copy_from_slice(&(0x0200_0000u32 + len as u32).to_le_bytes());
    k
}

/// A root image with a wholly-0xff block in the middle and 0xff at the end
/// of its last block, to exercise the trimming.
fn rootfs(seed: u32, len: usize) -> Vec<u8> {
    let mut r = prng(seed, len);
    let b = BLOCK as usize;
    if len > 3 * b {
        r[b..2 * b].fill(0xff);
    }
    let tail = len.saturating_sub(700);
    r[tail..len - 1].fill(0xff);
    r[len - 1] = 0x5a;
    r
}

struct Image {
    kernel: Vec<u8>,
    rootfs: Vec<u8>,
    header: AppHeader,
    eff: Vec<u8>,
}

fn image(model: &str, version: &str, seed: u32, klen: usize, rlen: usize) -> Image {
    let (k, r) = (kernel(seed, klen), rootfs(seed ^ 0x5555, rlen));
    let name = format!("{version}_{model}_firmware.bin");
    let ota = eff::ota::pack(&k, &r, "Fri Nov 13 12:00:00 2026", &name).unwrap();
    let header = eff::ota::Ota::parse(&ota).unwrap().header.app;
    let eff = eff::envelope::pack(&EffHeader::new(model, version).unwrap(), &ota, [seed as u8; 8]).unwrap();
    Image { kernel: k, rootfs: r, header, eff }
}

const CMD_A: &str = "console=ttyAMA0,115200n8 digium_board=D60 root=/dev/mtdblock6 rootfstype=jffs2 ro bootmemheap";
const CMD_B: &str = "console=ttyAMA0,115200n8 digium_board=D60 root=/dev/mtdblock7 rootfstype=jffs2 ro bootmemheap";

fn repl_of(model: Model) -> Replication {
    ctrl::replication_for(model.family())
}

/// A phone with `a` in bank A (active) and `b` in bank B.
fn phone(model: Model, a: &Image, b: &Image, active: Bank) -> MemNand {
    let nand = MemNand::new();
    nand.load("k0", &a.kernel);
    nand.load("root0", &a.rootfs);
    nand.load("k1", &b.kernel);
    nand.load("root1", &b.rootfs);
    let rec = CtrlRecord {
        active: bank::index(active) as u32,
        sides: [Side::from_header(&a.header), Side::from_header(&b.header)],
    };
    let mut m = nand.clone();
    ctrl::write(m.open("ctrl").unwrap().as_mut(), &rec, repl_of(model)).unwrap();
    nand
}

/// The image bytes stored in a partition, read with bad-block skipping.
fn stored(nand: &MemNand, part: &str, len: usize) -> Vec<u8> {
    let mut m = nand.clone();
    let mut p = m.open(part).unwrap();
    let mut buf = vec![0u8; len];
    assert!(flash::read_skip_bad(p.as_mut(), &mut buf).unwrap());
    buf
}

/// The bank holds exactly this image, the way boot2 and JFFS2 will read
/// it, and its header in `ctrl` matches.
fn assert_bank_holds(nand: &MemNand, bank: Bank, img: &Image) {
    let mut m = nand.clone();
    let loaded = boot2::load_kernel(&mut m, bank).unwrap().expect("loadable kernel");
    assert_eq!(loaded, img.kernel, "kernel in bank {bank:?}");
    assert_eq!(stored(nand, bank::rootfs_part(bank), img.rootfs.len()), img.rootfs, "rootfs in bank {bank:?}");
    let mut r = m.open(bank::rootfs_part(bank)).unwrap();
    let crc = flash::readback_crc(r.as_mut(), img.rootfs.len() as u64, true, |_, _| {}).expect("erased past image");
    assert_eq!(crc, img.header.rootfs_crc);
    let rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
    assert_eq!(rec.sides[bank::index(bank)].header(), Some(img.header));
}

/// Installs with a trial directory of its own: the tests of the flash
/// side install again and again, which a pending trial forbids.
fn install(nand: &MemNand, model: Model, cmdline: &str, eff: &[u8]) -> crate::Result<Installed> {
    install_in(nand, model, cmdline, eff, tempfile::tempdir().unwrap().path())
}

/// Installs with the trial record (and the update lock) in `dir`.
fn install_in(nand: &MemNand, model: Model, cmdline: &str, eff: &[u8], dir: &std::path::Path) -> crate::Result<Installed> {
    let mut m = nand.clone();
    Updater::new(&mut m, model, cmdline, dir).install_eff(eff)
}

fn sizes() -> (usize, usize) {
    // Kernel: 3 blocks and a bit of 6; rootfs: 6 blocks and a bit of 10.
    (3 * BLOCK as usize + 1234, 6 * BLOCK as usize + 999)
}

// --------------------------------------------------------------- ctrl layout

#[test]
fn ctrl_layout_per_family() {
    let rec = CtrlRecord::parse(&d60_live()).unwrap();
    let (page, block) = (PAGE as usize, BLOCK as usize);
    for (repl, copies_per_block) in [(Replication::EveryPage, block / page), (Replication::BlockStart, 1)] {
        let nand = MemNand::new();
        nand.set_bad("ctrl", 1);
        let mut m = nand.clone();
        let mut part = m.open("ctrl").unwrap();
        ctrl::write(part.as_mut(), &rec, repl).unwrap();
        let raw = nand.contents("ctrl");
        for b in 0..4 {
            for p in 0..block / page {
                let at = b * block + p * page;
                let pg = &raw[at..at + page];
                if b != 1 && p < copies_per_block {
                    assert_eq!(&pg[..304], &d60_live()[..], "copy at block {b} page {p}");
                    assert!(pg[304..].iter().all(|x| *x == 0xff), "0xff padding to the page");
                } else {
                    assert!(pg.iter().all(|x| *x == 0xff), "block {b} page {p} stays erased");
                }
            }
        }
        let scan = ctrl::scan(part.as_mut()).unwrap();
        assert_eq!((scan.valid_copies, scan.good_blocks, scan.bad_blocks), (3, 3, 1));
        assert!(scan.consistent);
        assert_eq!(scan.record, Some(rec));
        // Rewriting the same record costs no erases.
        let erases = nand.erases("ctrl");
        ctrl::write(part.as_mut(), &rec, repl).unwrap();
        assert_eq!(nand.erases("ctrl"), erases);
    }
}

#[test]
fn ctrl_scan_with_no_valid_copy_follows_boot2() {
    let nand = MemNand::new();
    let mut m = nand.clone();
    let mut part = m.open("ctrl").unwrap();
    // Erased partition: boot2 carries on with the last page read; both
    // headers are invalid there, so side 0.
    let scan = ctrl::scan(part.as_mut()).unwrap();
    assert!(scan.record.is_none() && scan.fallback.is_some());
    assert_eq!(ctrl::boot_side(&scan), 0);
    assert!(matches!(ctrl::read(part.as_mut()), Err(UpdateError::Ctrl(_))));
    // First block garbage, second valid: the valid one is used.
    let rec = CtrlRecord::parse(&d60_live()).unwrap();
    let mut raw = vec![0xffu8; 4 * BLOCK as usize];
    raw[..304].copy_from_slice(&prng(7, 304));
    raw[BLOCK as usize..BLOCK as usize + 304].copy_from_slice(&rec.to_bytes());
    nand.load("ctrl", &raw);
    let scan = ctrl::scan(part.as_mut()).unwrap();
    assert_eq!((scan.record, scan.record_block), (Some(rec), Some(1)));
}

#[test]
fn ctrl_write_refuses_to_leave_a_stale_copy_in_front() {
    let (kl, rl) = sizes();
    let a = image("D60", "3_0_0", 1, kl, rl);
    let nand = phone(Model::D60, &a, &a, Bank::A);
    // Block 0 cannot be erased, so it keeps the old record, which boot2
    // reads first: the write must fail rather than report success.
    nand.fail_erases("ctrl", 0);
    let mut m = nand.clone();
    let mut part = m.open("ctrl").unwrap();
    let mut rec = ctrl::read(part.as_mut()).unwrap();
    rec.active = 1;
    assert!(matches!(ctrl::write(part.as_mut(), &rec, Replication::EveryPage), Err(UpdateError::Ctrl(_))));
    assert_eq!(ctrl::read(part.as_mut()).unwrap().active, 0);
}

// ------------------------------------------------------------------- install

#[test]
fn install_into_the_spare_bank_both_families_both_directions() {
    let (kl, rl) = sizes();
    for model in [Model::D60, Model::D40] {
        let name = model.to_string();
        let old = image(&name, "2_9_28", 1, kl - 100, rl - 5000);
        let older = image(&name, "2_9_27", 2, kl - 300, rl - 4000);
        let new = image(&name, "3_0_0", 3, kl, rl);
        for (cmdline, running, target) in [(CMD_A, Bank::A, Bank::B), (CMD_B, Bank::B, Bank::A)] {
            let nand = match running {
                Bank::A => phone(model, &old, &older, Bank::A),
                Bank::B => phone(model, &older, &old, Bank::B),
            };
            // Bad blocks in the target's kernel and root partitions and in ctrl.
            nand.set_bad(bank::kernel_part(target), 1);
            nand.set_bad(bank::rootfs_part(target), 0);
            nand.set_bad(bank::rootfs_part(target), 4);
            nand.set_bad("ctrl", 0);
            let store = Store::new(model);
            let mut phases = Vec::new();
            let mut m = nand.clone();
            let done = Updater::new(&mut m, model, cmdline, tempfile::tempdir().unwrap().path())
                .store(store.clone())
                .progress(|p| phases.push(p.clone()))
                .install_eff(&new.eff[..])
                .unwrap();
            assert_eq!(done.target, Target::Bank { bank: target, previous: running, header: new.header, bad_blocks: (1, 2) });
            assert_eq!((done.version.as_str(), done.is_stock), ("3_0_0", false));
            assert_bank_holds(&nand, target, &new);
            assert_bank_holds(&nand, running, &old);
            let boot = boot2::simulate(&mut m).unwrap();
            assert_eq!(boot.side, target);
            assert!(boot.kernel.is_ok());
            // ctrl in the family's layout, bad block skipped.
            let scan = ctrl::scan(m.open("ctrl").unwrap().as_mut()).unwrap();
            assert_eq!((scan.valid_copies, scan.bad_blocks, scan.consistent), (3, 1, true));
            let raw = nand.contents("ctrl");
            let second_page = &raw[BLOCK as usize + PAGE as usize..BLOCK as usize + 2 * PAGE as usize];
            match repl_of(model) {
                Replication::EveryPage => assert_eq!(&second_page[..304], &scan.record.unwrap().to_bytes()[..]),
                Replication::BlockStart => assert!(second_page.iter().all(|b| *b == 0xff)),
            }
            // Progress: ordered phases ending in Complete, mirrored in the store.
            assert_eq!(phases.first(), Some(&UpdatePhase::Checking));
            assert_eq!(phases.last(), Some(&UpdatePhase::Complete));
            let pos = |f: fn(&UpdatePhase) -> bool| phases.iter().position(f).unwrap();
            assert!(pos(|p| matches!(p, UpdatePhase::WritingKernel { .. })) < pos(|p| matches!(p, UpdatePhase::WritingRootfs { .. })));
            assert!(pos(|p| matches!(p, UpdatePhase::WritingRootfs { percent: 100 })) < pos(|p| matches!(p, UpdatePhase::Verifying { .. })));
            assert!(pos(|p| matches!(p, UpdatePhase::Verifying { .. })) < pos(|p| *p == UpdatePhase::Committing));
            let u = store.get::<Update>();
            assert_eq!(u.phase, UpdatePhase::Complete);
            assert_eq!((u.incoming_version.as_str(), u.incoming_is_stock), ("3_0_0", false));
        }
    }
}

#[test]
fn stock_images_install_and_are_flagged() {
    let (kl, rl) = sizes();
    let ours = image("D60", "3_0_0", 1, kl, rl);
    let stock = image("D60", "2_9_28", 2, kl, rl);
    let nand = phone(Model::D60, &ours, &ours, Bank::A);
    // No trial for stock, and a record left over from an earlier trial
    // (the phone is back on the bank to return to) is cleared.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(trial::record_path(dir.path()), "pending=1\nprevious_bank=0\n").unwrap();
    let done = install_in(&nand, Model::D60, CMD_A, &stock.eff, dir.path()).unwrap();
    assert!(done.is_stock);
    assert_bank_holds(&nand, Bank::B, &stock);
    assert_eq!(trial::load(dir.path()), None);
}

#[test]
fn trailing_ff_is_not_programmed_but_reads_back() {
    // A kernel whose last block is all 0xff after the first page.
    let (_, rl) = sizes();
    let mut img = image("D60", "3_0_0", 9, 2 * BLOCK as usize, rl);
    let b = BLOCK as usize;
    img.kernel[b + 100..].fill(0xff);
    let ota = eff::ota::pack(&img.kernel, &img.rootfs, "d", "3_0_0_D60_firmware.bin").unwrap();
    img.header = eff::ota::Ota::parse(&ota).unwrap().header.app;
    img.eff = eff::envelope::pack(&EffHeader::new("D60", "3_0_0").unwrap(), &ota, [1; 8]).unwrap();
    let old = image("D60", "2_9_28", 1, 5000, 9000);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    install(&nand, Model::D60, CMD_A, &img.eff).unwrap();
    assert_bank_holds(&nand, Bank::B, &img);
}

fn assert_refused_untouched(model: Model, cmdline: &str, nand: &MemNand, eff: &[u8], opts: Options) -> UpdateError {
    let before = nand.snapshot();
    let ops = nand.ops();
    let mut m = nand.clone();
    let err = Updater::new(&mut m, model, cmdline, tempfile::tempdir().unwrap().path()).options(opts).install_eff(eff).unwrap_err();
    assert_eq!(nand.ops(), ops, "no flash operation: {err}");
    assert_eq!(nand.snapshot(), before);
    err
}

#[test]
fn refusals_leave_the_flash_untouched() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let none = Options::default;

    // Another model's image.
    let d62 = image("D62", "3_0_0", 3, kl, rl);
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &d62.eff, none());
    assert!(matches!(e, UpdateError::WrongModel { .. }), "{e}");

    // The running bank named as the target.
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff, Options { target: Some(Bank::A), ..none() });
    assert!(matches!(e, UpdateError::Refused(_)), "{e}");

    // Not an EFF at all; a truncated header.
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &prng(5, 4000), none());
    assert!(matches!(e, UpdateError::Image(_)), "{e}");
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff[..300], none());
    assert!(matches!(e, UpdateError::Image(_)), "{e}");

    // A kernel that boot2 would copy over itself (unpatched zImage header).
    let mut k = kernel(4, kl);
    k[0x28..0x2c].copy_from_slice(&0u32.to_le_bytes());
    k[0x2c..0x30].copy_from_slice(&(kl as u32).to_le_bytes());
    let ota = eff::ota::pack(&k, &new.rootfs, "d", "n").unwrap();
    let bad = eff::envelope::pack(&EffHeader::new("D60", "3_0_0").unwrap(), &ota, [2; 8]).unwrap();
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &bad, none());
    assert!(matches!(&e, UpdateError::Image(m) if m.contains("header-patched")), "{e}");

    // Not a kernel.
    let ota = eff::ota::pack(&prng(6, kl), &new.rootfs, "d", "n").unwrap();
    let bad = eff::envelope::pack(&EffHeader::new("D60", "3_0_0").unwrap(), &ota, [2; 8]).unwrap();
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &bad, none());
    assert!(matches!(&e, UpdateError::Image(m) if m.contains("zImage")), "{e}");

    // A bootstrap update (OTA type 1), which would rewrite the boot loader:
    // a well-formed type header (its own CRC is right) in front of the
    // image, inside a good EFF for this model.
    let mut ota = eff::ota::pack(&new.kernel, &new.rootfs, "d", "n").unwrap();
    ota[4..8].copy_from_slice(&1u32.to_be_bytes());
    ota[8..12].copy_from_slice(&eff::ota::type_crc(eff::ota::OTA_MAGIC, 1).to_be_bytes());
    let bootstrap = eff::envelope::pack(&EffHeader::new("D60", "3_0_0").unwrap(), &ota, [2; 8]).unwrap();
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &bootstrap, none());
    assert!(matches!(&e, UpdateError::Image(m) if m.contains("OTA type 1")), "{e}");

    // Too large once bad blocks are counted: 6-block slot, 3 bad, kernel
    // needs 4.
    for b in [0, 2, 5] {
        nand.set_bad("k1", b);
    }
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff, none());
    assert!(matches!(e, UpdateError::Layout(_)), "{e}");
}

#[test]
fn refuses_when_the_bank_that_stays_is_not_bootable() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);

    // Running from A, but A's header is invalid in ctrl.
    let nand = phone(Model::D60, &old, &old, Bank::B);
    let mut m = nand.clone();
    let mut rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
    rec.sides[0] = Side::INVALID;
    ctrl::write(m.open("ctrl").unwrap().as_mut(), &rec, Replication::EveryPage).unwrap();
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff, Options::default());
    assert!(matches!(e, UpdateError::Refused(_)), "{e}");

    // A's kernel slot is erased.
    let nand = phone(Model::D60, &old, &old, Bank::A);
    nand.load("k0", &[]);
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff, Options::default());
    assert!(matches!(e, UpdateError::Refused(_)), "{e}");
    // ...unless told to go ahead (recovery from a RAM boot).
    let mut m = nand.clone();
    Updater::new(&mut m, Model::D60, CMD_A, tempfile::tempdir().unwrap().path())
        .options(Options { allow_unbootable_other: true, ..Default::default() })
        .install_eff(&new.eff[..])
        .unwrap();
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::B);

    // No valid ctrl record at all.
    let nand = phone(Model::D60, &old, &old, Bank::A);
    nand.load("ctrl", &[]);
    let e = assert_refused_untouched(Model::D60, CMD_A, &nand, &new.eff, Options::default());
    assert!(matches!(e, UpdateError::Ctrl(_)), "{e}");
}

#[test]
fn ram_boot_targets_the_bank_boot2_would_not_boot() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::B);
    let done = install(&nand, Model::D60, "root=/dev/mtdblock8 ro", &new.eff).unwrap();
    assert!(matches!(done.target, Target::Bank { bank: Bank::A, previous: Bank::B, .. }), "{:?}", done.target);
    assert_bank_holds(&nand, Bank::A, &new);
}

/// A reader that corrupts one byte at `at`, or ends at `end`.
struct Tamper<'a> {
    data: &'a [u8],
    pos: usize,
    flip: Option<usize>,
    end: usize,
}

impl Read for Tamper<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.end - self.pos).min(1000);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        if let Some(at) = self.flip
            && (self.pos..self.pos + n).contains(&at)
        {
            buf[at - self.pos] ^= 0x01;
        }
        self.pos += n;
        Ok(n)
    }
}

#[test]
fn corrupt_or_truncated_streams_never_commit() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let len = new.eff.len();
    // AES-CBC: a flipped ciphertext bit garbles its block and flips a bit
    // in the next, so every case below is a CRC or padding failure.
    let cases: Vec<(Option<usize>, usize)> = vec![
        (Some(116 + 400 + 160 + 5000), len),       // in the kernel
        (Some(116 + 400 + 160 + kl + 9000), len),  // in the rootfs
        (Some(len - 20), len),                     // in the last blocks
        (None, len - 16),                          // one AES block short
        (None, len / 2),                           // cut in the rootfs
        (None, 116 + 400 + 160 + 2000),            // cut in the kernel
    ];
    for (flip, end) in cases {
        let nand = phone(Model::D60, &old, &old, Bank::A);
        let store = Store::new(Model::D60);
        let mut m = nand.clone();
        let ops = nand.ops();
        let err = Updater::new(&mut m, Model::D60, CMD_A, tempfile::tempdir().unwrap().path())
            .store(store.clone())
            .install_eff(Tamper { data: &new.eff, pos: 0, flip, end })
            .unwrap_err();
        assert!(matches!(err, UpdateError::Image(_)), "{flip:?} {end}: {err}");
        assert!(matches!(store.get::<Update>().phase, UpdatePhase::Failed(_)));
        let boot = boot2::simulate(&mut m).unwrap();
        assert_eq!(boot.side, Bank::A);
        assert_bank_holds(&nand, Bank::A, &old);
        let rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
        // Either the stream failed before the flash was touched (a cut
        // inside the first kernel block), or the target stays invalid.
        assert!(nand.ops() == ops || !rec.sides[1].is_valid(), "target stays invalid ({flip:?} {end})");
        // And a good image installs afterwards.
        install(&nand, Model::D60, CMD_A, &new.eff).unwrap();
        assert_bank_holds(&nand, Bank::B, &new);
    }
}

#[test]
fn block_failures_stop_the_update_or_mark_bad_when_asked() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    for (part, write_fault) in [("k1", true), ("root1", true), ("root1", false)] {
        // Default: fail, nothing marked, old bank boots.
        let nand = phone(Model::D60, &old, &old, Bank::A);
        if write_fault { nand.corrupt_writes(part, 2) } else { nand.fail_erases(part, 2) }
        let err = install(&nand, Model::D60, CMD_A, &new.eff).unwrap_err();
        assert!(matches!(err, UpdateError::Verify(_) | UpdateError::Hal(_)), "{err}");
        assert!(!nand.is_bad(part, 2));
        let mut m = nand.clone();
        assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
        assert_bank_holds(&nand, Bank::A, &old);

        // Opt-in: the block is marked bad and the image lands around it.
        let nand = phone(Model::D60, &old, &old, Bank::A);
        if write_fault { nand.corrupt_writes(part, 2) } else { nand.fail_erases(part, 2) }
        let mut m = nand.clone();
        Updater::new(&mut m, Model::D60, CMD_A, tempfile::tempdir().unwrap().path())
            .options(Options { mark_bad_on_failure: true, ..Default::default() })
            .install_eff(&new.eff[..])
            .unwrap();
        assert!(nand.is_bad(part, 2));
        assert_bank_holds(&nand, Bank::B, &new);
    }
}

// ----------------------------------------------------------- trial, rollback

#[test]
fn trial_and_rollback() {
    let (kl, rl) = sizes();
    for model in [Model::D60, Model::D70] {
        let name = model.to_string();
        let old = image(&name, "2_9_28", 1, kl, rl);
        let new = image(&name, "3_0_0", 3, kl, rl);
        let nand = phone(model, &old, &old, Bank::A);
        let dir = tempfile::tempdir().unwrap();
        install_in(&nand, model, CMD_A, &new.eff, dir.path()).unwrap();
        let rec = trial::load(dir.path()).unwrap();
        assert_eq!((rec.previous_bank, rec.trial_bank, rec.boots), (Bank::A, Some(Bank::B), 0));

        let mut m = nand.clone();
        let store = Store::new(model);
        // Rebooted into B: the state shows the pending trial.
        trial::seed_state(&store, dir.path(), CMD_B, &m);
        let u = store.get::<Update>();
        assert_eq!(u.running_bank, Some(Bank::B));
        assert_eq!(u.trial, opendeskphone_state::TrialBoot::Pending { boots_left: 3 });

        // A stale record (we are already on the previous bank) is refused.
        let before = nand.snapshot();
        assert!(matches!(trial::rollback(&mut m, model, CMD_A, Bank::A), Err(UpdateError::Refused(_))));
        assert_eq!(nand.snapshot(), before);

        // The real thing: running B, go back to A.
        trial::rollback(&mut m, model, CMD_B, Bank::A).unwrap();
        let boot = boot2::simulate(&mut m).unwrap();
        assert_eq!(boot.side, Bank::A);
        assert_bank_holds(&nand, Bank::A, &old);
        // The failed image's header is kept for inspection.
        assert_bank_holds(&nand, Bank::B, &new);

        // Healthy instead: record cleared, state updated.
        trial::mark_healthy(dir.path(), Some(&store)).unwrap();
        assert_eq!(trial::load(dir.path()), None);
        assert_eq!(store.get::<Update>().trial, opendeskphone_state::TrialBoot::None);
    }
}

/// `arm-trial`: the bank on trial is the one `ctrl` selects, and the bank
/// to return to is the other one; `root=` only confirms it.
#[test]
fn arm_trial_follows_ctrl_and_refuses_what_it_cannot_tell() {
    let (kl, rl) = sizes();
    let stock = image("D40", "2_9_26", 1, kl, rl);
    let open = image("D40", "3_0_0", 3, kl, rl);
    let refused = |r: crate::Result<trial::TrialRecord>| match r {
        Err(UpdateError::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    };
    let armed = |nand: &MemNand, cmdline: &str, return_to: Option<Bank>| {
        let dir = tempfile::tempdir().unwrap();
        let mut m = nand.clone();
        let before = nand.snapshot();
        let r = trial::arm(&mut m, cmdline, dir.path(), return_to);
        assert_eq!(nand.snapshot(), before, "arming never writes flash");
        // What is returned is what is stored, or nothing is stored.
        assert_eq!(trial::load(dir.path()), r.as_ref().ok().cloned());
        r
    };

    // Booted from flash in A (stock there in this fixture is only a
    // bootable image), B installed and selected by other means.
    let nand = phone(Model::D40, &stock, &open, Bank::B);
    let r = armed(&nand, CMD_A, None).unwrap();
    assert_eq!((r.previous_bank, r.trial_bank, r.boots, r.max_boots), (Bank::A, Some(Bank::B), 0, 3));
    // Saying the same thing explicitly changes nothing.
    assert_eq!(armed(&nand, CMD_A, Some(Bank::A)).unwrap(), r);
    // The selected bank cannot be the one to return to.
    let m = refused(armed(&nand, CMD_A, Some(Bank::B)));
    assert!(m.contains("cannot also be the bank to return to"), "{m}");

    // The D40 bring-up: the kernel was started from RAM with root1 as its
    // root, and ctrl then selected B.  root= names the bank on trial, so
    // the old rule ("the running bank is the one to return to") would have
    // recorded a return to B and a trial of A.
    let m = refused(armed(&nand, CMD_B, None));
    assert!(m.contains("--return-to a"), "{m}");
    let r = armed(&nand, CMD_B, Some(Bank::A)).unwrap();
    assert_eq!((r.previous_bank, r.trial_bank), (Bank::A, Some(Bank::B)));
    // The record then does its work on the boot through boot2: the state
    // shows the trial, and the rollback goes to A.
    let dir = tempfile::tempdir().unwrap();
    let mut m2 = nand.clone();
    trial::arm(&mut m2, CMD_B, dir.path(), Some(Bank::A)).unwrap();
    let store = Store::new(Model::D40);
    trial::seed_state(&store, dir.path(), CMD_B, &m2);
    assert_eq!(store.get::<Update>().trial, opendeskphone_state::TrialBoot::Pending { boots_left: 3 });
    trial::rollback(&mut m2, Model::D40, CMD_B, r.previous_bank).unwrap();
    assert_eq!(boot2::simulate(&mut m2).unwrap().side, Bank::A);

    // The same RAM boot before ctrl was switched: boot2 boots A next, which
    // holds stock, and stock does not count trial boots.
    let nand = phone(Model::D40, &stock, &open, Bank::A);
    let m = refused(armed(&nand, CMD_B, None));
    assert!(m.contains("stock firmware 2_9_26"), "{m}");

    // Booted from flash in the selected bank: nothing is on trial.
    let nand = phone(Model::D40, &open, &open, Bank::A);
    let m = refused(armed(&nand, CMD_A, None));
    assert!(m.contains("nothing is on trial") && m.contains("--return-to b"), "{m}");

    // Root in neither bank (e.g., userdata): the bank to return to must be named.
    let m = refused(armed(&nand, "root=/dev/mtdblock8 ro", None));
    assert!(m.contains("names neither bank") && m.contains("--return-to b"), "{m}");
    let r = armed(&nand, "root=/dev/mtdblock8 ro", Some(Bank::B)).unwrap();
    assert_eq!((r.previous_bank, r.trial_bank), (Bank::B, Some(Bank::A)));

    // A bank to return to that boot2 could not boot is no way back.
    let nand = phone(Model::D40, &open, &open, Bank::B);
    nand.load("k0", &[]);
    let m = refused(armed(&nand, CMD_A, None));
    assert!(m.contains("bank A (to return to) is not bootable"), "{m}");

    // No valid ctrl record: nothing was selected.
    let empty = MemNand::new();
    let m = refused(armed(&empty, CMD_A, Some(Bank::B)));
    assert!(m.contains("no valid record"), "{m}");
}

#[test]
fn select_bank_refuses_an_unbootable_bank() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let mut m = nand.clone();
    // B's header invalid.
    let mut rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
    rec.sides[1] = Side::INVALID;
    ctrl::write(m.open("ctrl").unwrap().as_mut(), &rec, Replication::EveryPage).unwrap();
    let before = nand.snapshot();
    assert!(matches!(select_bank(&mut m, Model::D60, Bank::B), Err(UpdateError::Refused(_))));
    assert_eq!(nand.snapshot(), before);
    // B's header valid but its kernel gone.
    let nand = phone(Model::D60, &old, &old, Bank::A);
    nand.load("k1", &[]);
    let mut m = nand.clone();
    assert!(matches!(select_bank(&mut m, Model::D60, Bank::B), Err(UpdateError::Refused(_))));
}

// ------------------------------------------------------------------- inspect

#[test]
fn inspect_reports_banks_and_next_boot() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    nand.set_bad("root1", 3);
    let mut m = nand.clone();
    let s = inspect(&mut m, CMD_A, None).unwrap();
    assert_eq!((s.running, s.active, s.next_boot), (Some(Bank::A), Some(Bank::A), Bank::A));
    assert!(s.ctrl_valid && s.ctrl_consistent);
    assert_eq!(s.ctrl_copies, (4, 4, 0));
    assert_eq!(s.banks[0].version.as_deref(), Some("2_9_28"));
    assert!(s.banks[0].bootable() && s.banks[1].bootable());
    assert_eq!(s.banks[1].rootfs_bad_blocks, 1);

    let dir = tempfile::tempdir().unwrap();
    install_in(&nand, Model::D60, CMD_A, &new.eff, dir.path()).unwrap();
    let s = inspect(&mut m, CMD_A, Some(dir.path())).unwrap();
    assert_eq!((s.running, s.active, s.next_boot), (Some(Bank::A), Some(Bank::B), Bank::B));
    assert_eq!(s.banks[1].version.as_deref(), Some("3_0_0"));
    assert_eq!(s.banks[1].image_name, "3_0_0_D60_firmware.bin");
    assert_eq!(s.trial.unwrap().previous_bank, Bank::A);
    let (ops, before) = (nand.ops(), nand.snapshot());
    inspect(&mut m, CMD_A, Some(dir.path())).unwrap();
    assert_eq!((nand.ops(), nand.snapshot()), (ops, before), "inspect never writes");

    // Inspecting an erased flash works and reports nothing bootable.
    let empty = MemNand::new();
    let ops = empty.ops();
    let mut m = empty.clone();
    let s = inspect(&mut m, "", None).unwrap();
    assert!(!s.ctrl_valid && s.running.is_none() && s.active.is_none());
    assert_eq!(s.next_boot, Bank::A);
    assert!(!s.banks[0].bootable() && !s.banks[1].bootable());
    assert_eq!(empty.ops(), ops, "inspect never writes");
}

// ----------------------------------------------------------------- power cuts

/// After a power cut anywhere, boot2 must pick a bank that holds a
/// complete image: the old one in the kept bank, or (only once committed)
/// the new one in the target.
fn assert_boots_whole_image(nand: &MemNand, keep: (&Image, Bank), new: (&Image, Bank), ctx: &str) -> Bank {
    let mut m = nand.clone();
    let boot = boot2::simulate(&mut m).unwrap();
    assert!(boot.kernel.is_ok(), "{ctx}: boot2 would not load a kernel: {:?}", boot.kernel);
    let (img, bank) = if boot.side == keep.1 { keep } else { new };
    assert_eq!(boot.side, bank);
    let loaded = boot2::load_kernel(&mut m, bank).unwrap().unwrap();
    assert_eq!(loaded, img.kernel, "{ctx}: kernel of the bank boot2 picked");
    assert_eq!(stored(nand, bank::rootfs_part(bank), img.rootfs.len()), img.rootfs, "{ctx}: rootfs");
    if bank == new.1 {
        // A committed target is clean past the image too (JFFS2 scans it all).
        let mut r = m.open(bank::rootfs_part(bank)).unwrap();
        flash::readback_crc(r.as_mut(), img.rootfs.len() as u64, true, |_, _| {})
            .unwrap_or_else(|e| panic!("{ctx}: {e}"));
    }
    boot.side
}

#[test]
fn power_cut_at_every_flash_operation_during_install() {
    let (kl, rl) = sizes();
    for model in [Model::D60, Model::D40] {
        let name = model.to_string();
        let old = image(&name, "2_9_28", 1, kl - 100, rl - 5000);
        let older = image(&name, "2_9_27", 2, kl - 300, rl - 4000);
        let new = image(&name, "3_0_0", 3, kl, rl);
        let fresh = || {
            let n = phone(model, &old, &older, Bank::A);
            n.set_bad("k1", 2);
            n.set_bad("root1", 1);
            n
        };
        // How many flash operations a whole install takes.
        let total = {
            let n = fresh();
            let start = n.ops();
            install(&n, model, CMD_A, &new.eff).unwrap();
            n.ops() - start
        };
        assert!(total > 20, "{total}");
        let mut booted_new = 0;
        for cut in 0..total {
            for seed in 1..=3u64 {
                let nand = fresh();
                nand.cut_after(cut, seed);
                let r = install(&nand, model, CMD_A, &new.eff);
                assert!(nand.is_dead(), "cut {cut} did not fire");
                assert!(r.is_err(), "an interrupted install must not report success (cut {cut})");
                nand.power_on();
                let ctx = format!("{model} cut {cut}/{total} seed {seed}");
                let side = assert_boots_whole_image(&nand, (&old, Bank::A), (&new, Bank::B), &ctx);
                if side == Bank::B {
                    booted_new += 1;
                } else {
                    // Still on the old bank: installing again must work
                    // from whatever state the cut left.
                    install(&nand, model, CMD_A, &new.eff).unwrap_or_else(|e| panic!("{ctx}: retry: {e}"));
                    assert_bank_holds(&nand, Bank::B, &new);
                    assert_bank_holds(&nand, Bank::A, &old);
                    let mut m = nand.clone();
                    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::B);
                }
            }
        }
        // Cuts inside the final ctrl rewrite land on the new image.
        assert!(booted_new > 0, "{model}: no cut landed after the commit point");
    }
}

#[test]
fn power_cut_at_every_flash_operation_during_rollback() {
    let (kl, rl) = sizes();
    for model in [Model::D60, Model::D40] {
        let name = model.to_string();
        let old = image(&name, "2_9_28", 1, kl, rl);
        let new = image(&name, "3_0_0", 3, kl, rl);
        let fresh = || {
            let n = phone(model, &old, &old, Bank::A);
            install(&n, model, CMD_A, &new.eff).unwrap();
            n
        };
        let total = {
            let n = fresh();
            let start = n.ops();
            let mut m = n.clone();
            trial::rollback(&mut m, model, CMD_B, Bank::A).unwrap();
            n.ops() - start
        };
        assert!(total >= 8, "{total}");
        for cut in 0..total {
            for seed in 1..=3u64 {
                let nand = fresh();
                nand.cut_after(cut, seed);
                let mut m = nand.clone();
                assert!(trial::rollback(&mut m, model, CMD_B, Bank::A).is_err());
                nand.power_on();
                let ctx = format!("{model} rollback cut {cut}/{total} seed {seed}");
                assert_boots_whole_image(&nand, (&old, Bank::A), (&new, Bank::B), &ctx);
                // Rolling back again completes.
                trial::rollback(&mut m, model, CMD_B, Bank::A).unwrap_or_else(|e| panic!("{ctx}: retry: {e}"));
                assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
            }
        }
    }
}

#[test]
fn memnand_obeys_nand_rules() {
    // Guard the test double itself: programming only clears bits.
    let nand = MemNand::new();
    let mut m = nand.clone();
    let mut p = m.open("k0").unwrap();
    p.write(0, &[0x0f; 512]).unwrap();
    p.write(0, &[0xf0; 512]).unwrap();
    let mut b = [0u8; 4];
    p.read(0, &mut b).unwrap();
    assert_eq!(b, [0; 4]);
    p.erase(0).unwrap();
    p.read(0, &mut b).unwrap();
    assert_eq!(b, [0xff; 4]);
}

// ------------------------------------------- one update, verified, on trial

/// A reader that calls `at_half` once, when half of the data has been read.
struct Midway<'a, F: FnMut()> {
    data: &'a [u8],
    pos: usize,
    at_half: Option<F>,
}

impl<F: FnMut()> Read for Midway<'_, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.data.len() / 2
            && let Some(mut f) = self.at_half.take()
        {
            f();
        }
        let n = buf.len().min(self.data.len() - self.pos).min(1000);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[test]
fn one_update_at_a_time() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let other = image("D60", "3_0_1", 4, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let dir = tempfile::tempdir().unwrap();
    let mut m = nand.clone();
    // While the first update is writing, everything else that would write
    // a bank or ctrl is refused, and does nothing to the flash.
    let meanwhile = || {
        let (ops, before) = (nand.ops(), nand.snapshot());
        let e = install_in(&nand, Model::D60, CMD_A, &other.eff, dir.path()).unwrap_err();
        assert!(matches!(&e, UpdateError::Refused(m) if m.contains("another firmware update")), "{e}");
        let e = Updater::new(&mut nand.clone(), Model::D60, CMD_A, dir.path())
            .install_eff_file(std::io::Cursor::new(&other.eff))
            .unwrap_err();
        assert!(matches!(e, UpdateError::Refused(_)), "{e}");
        assert!(matches!(abandon(&mut nand.clone(), Model::D60, dir.path()), Err(UpdateError::Refused(_))));
        assert!(matches!(crate::lock::acquire(dir.path()), Err(UpdateError::Refused(_))));
        assert_eq!((nand.ops(), nand.snapshot()), (ops, before));
    };
    let reader = Midway { data: &new.eff, pos: 0, at_half: Some(meanwhile) };
    Updater::new(&mut m, Model::D60, CMD_A, dir.path()).install_eff(reader).unwrap();
    assert_bank_holds(&nand, Bank::B, &new);
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::B);
    // The lock went with the update.
    drop(crate::lock::acquire(dir.path()).unwrap());
}

#[test]
fn no_update_while_an_image_is_on_trial_until_it_is_abandoned() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let other = image("D60", "3_0_1", 4, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let dir = tempfile::tempdir().unwrap();
    let mut m = nand.clone();
    assert_eq!(abandon(&mut m, Model::D60, dir.path()).unwrap(), None);
    install_in(&nand, Model::D60, CMD_A, &new.eff, dir.path()).unwrap();
    // Before the reboot (still running A) and after it (running B on
    // trial): no other image, as a stream or as a file.
    for cmdline in [CMD_A, CMD_B] {
        let (ops, before) = (nand.ops(), nand.snapshot());
        let e = install_in(&nand, Model::D60, cmdline, &other.eff, dir.path()).unwrap_err();
        assert!(matches!(&e, UpdateError::Refused(m) if m.contains("dphone update abandon")), "{e}");
        let e = Updater::new(&mut m, Model::D60, cmdline, dir.path())
            .install_eff_file(std::io::Cursor::new(&other.eff))
            .unwrap_err();
        assert!(matches!(e, UpdateError::Refused(_)), "{e}");
        assert_eq!((nand.ops(), nand.snapshot()), (ops, before), "refused before any flash operation");
        assert_eq!(trial::load(dir.path()).unwrap().boots, 0, "the record is as it was");
    }
    // Giving it up selects the bank to return to and removes the record.
    assert_eq!(abandon(&mut m, Model::D60, dir.path()).unwrap(), Some(Bank::A));
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
    assert_eq!(trial::load(dir.path()), None);
    assert_bank_holds(&nand, Bank::A, &old);
    // Then the other image installs.
    install_in(&nand, Model::D60, CMD_A, &other.eff, dir.path()).unwrap();
    assert_bank_holds(&nand, Bank::B, &other);
    // A confirmed trial (record removed by the init role) does not stand
    // in the way either: the phone now runs B and writes A.
    trial::clear(dir.path()).unwrap();
    install_in(&nand, Model::D60, CMD_B, &new.eff, dir.path()).unwrap();
    assert_bank_holds(&nand, Bank::A, &new);
    assert_eq!(trial::load(dir.path()).unwrap().previous_bank, Bank::B);
}

/// The incident of 2026-11-14 as far as one update goes: a truncated file
/// (the copy to a full `/tmp` had failed) must cost nothing.
#[test]
fn a_bad_file_touches_neither_the_flash_nor_the_trial_record() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let len = new.eff.len();
    let cases: Vec<(Option<usize>, usize)> = vec![
        (Some(116 + 400 + 160 + 5000), len),      // a flipped bit in the kernel
        (Some(116 + 400 + 160 + kl + 9000), len), // in the rootfs
        (Some(len - 20), len),                    // in the last blocks
        (None, len - 16),                         // one AES block short
        (None, len / 2),                          // cut in the rootfs
        (None, 116 + 400 + 160 + 2000),           // cut in the kernel
        (None, 100),                              // cut in the envelope
    ];
    for (flip, end) in cases {
        let nand = phone(Model::D60, &old, &old, Bank::A);
        let dir = tempfile::tempdir().unwrap();
        let mut file = new.eff[..end].to_vec();
        if let Some(at) = flip {
            file[at] ^= 0x01;
        }
        assert!(matches!(check_eff(&file[..], Model::D60), Err(UpdateError::Image(_))));
        let (ops, before) = (nand.ops(), nand.snapshot());
        let mut m = nand.clone();
        let err = Updater::new(&mut m, Model::D60, CMD_A, dir.path())
            .install_eff_file(std::io::Cursor::new(&file))
            .unwrap_err();
        assert!(matches!(err, UpdateError::Image(_)), "{flip:?} {end}: {err}");
        assert_eq!(nand.ops(), ops, "no flash operation ({flip:?} {end})");
        assert_eq!(nand.snapshot(), before);
        assert_eq!(trial::load(dir.path()), None);
        assert!(!trial::record_path(dir.path()).exists());
    }
    // An image for another model, too; and the good file installs.
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let dir = tempfile::tempdir().unwrap();
    let mut m = nand.clone();
    let wrong = image("D40", "3_0_0", 3, kl, rl);
    let ops = nand.ops();
    let err = Updater::new(&mut m, Model::D60, CMD_A, dir.path())
        .install_eff_file(std::io::Cursor::new(&wrong.eff))
        .unwrap_err();
    assert!(matches!(err, UpdateError::WrongModel { .. }), "{err}");
    assert_eq!(nand.ops(), ops);
    // A D80 has no banks: nothing is read or written for an attempt on one.
    assert!(Updater::new(&mut m, Model::D80, CMD_A, dir.path()).install_eff_file(std::io::Cursor::new(&new.eff)).is_err());
    assert_eq!(nand.ops(), ops);
    assert_eq!(check_eff(&new.eff[..], Model::D60).unwrap(), "3_0_0");
    Updater::new(&mut m, Model::D60, CMD_A, dir.path()).install_eff_file(std::io::Cursor::new(&new.eff)).unwrap();
    assert_bank_holds(&nand, Bank::B, &new);
    assert_eq!(trial::load(dir.path()).unwrap().trial_bank, Some(Bank::B));
}

/// A stream cannot be read twice: a bad one costs the spare bank's old
/// contents, and nothing else.
#[test]
fn a_bad_stream_leaves_the_selection_and_the_trial_record_alone() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let dir = tempfile::tempdir().unwrap();
    let mut m = nand.clone();
    let before = nand.snapshot();
    let err = Updater::new(&mut m, Model::D60, CMD_A, dir.path())
        .install_eff(Tamper { data: &new.eff, pos: 0, flip: None, end: new.eff.len() / 2 })
        .unwrap_err();
    assert!(matches!(err, UpdateError::Image(_)), "{err}");
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
    assert_bank_holds(&nand, Bank::A, &old);
    assert!(!trial::record_path(dir.path()).exists());
    let rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
    assert!(!rec.sides[1].is_valid(), "the half-written bank cannot be selected");
    // Nothing but the spare bank and its header in ctrl changed.
    let after = nand.snapshot();
    for (i, (name, _)) in crate::memnand::PARTS.iter().enumerate() {
        if !["ctrl", "k1", "root1"].contains(name) {
            assert_eq!(after[i], before[i], "{name} changed");
        }
    }
}

/// The bank is read back and compared with the image after the last write
/// and before `ctrl` selects it: flash that changes in between is caught.
#[test]
fn flash_corrupted_after_writing_is_caught_before_the_bank_is_selected() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    for (part, offset) in [("k1", 2 * BLOCK as usize + 77), ("root1", 4 * BLOCK as usize + 5), ("root1", rl + 2000)] {
        let nand = phone(Model::D60, &old, &old, Bank::A);
        let dir = tempfile::tempdir().unwrap();
        let mut m = nand.clone();
        let mut hit = false;
        let err = Updater::new(&mut m, Model::D60, CMD_A, dir.path())
            .progress(|phase| {
                // Every block has been written and compared by now.
                if matches!(phase, UpdatePhase::Verifying { percent: 0 }) && !hit {
                    hit = true;
                    nand.flip_bit(part, offset);
                }
            })
            .install_eff(&new.eff[..])
            .unwrap_err();
        assert!(hit);
        assert!(matches!(err, UpdateError::Verify(_)), "{part} {offset}: {err}");
        assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
        let rec = ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap();
        assert!(!rec.sides[1].is_valid());
        assert!(!trial::record_path(dir.path()).exists());
    }
}

/// A web upload: written and verified during the upload, selected only
/// when the user confirms, and read back once more at that point.
#[test]
fn a_staged_image_is_selected_only_by_the_commit() {
    let (kl, rl) = sizes();
    let old = image("D60", "2_9_28", 1, kl, rl);
    let new = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &old, &old, Bank::A);
    let dir = tempfile::tempdir().unwrap();
    let mut m = nand.clone();
    let staged = Updater::new(&mut m, Model::D60, CMD_A, dir.path()).stage_eff(&new.eff[..]).unwrap();
    assert!(matches!(staged.target, Target::Bank { bank: Bank::B, previous: Bank::A, .. }), "{:?}", staged.target);
    assert_eq!(staged.version, "3_0_0");
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
    assert!(!ctrl::read(m.open("ctrl").unwrap().as_mut()).unwrap().sides[1].is_valid());
    assert!(!trial::record_path(dir.path()).exists());
    // The bank changed since: the commit refuses, and nothing is selected.
    nand.flip_bit("root1", 3 * BLOCK as usize);
    let err = Updater::new(&mut m, Model::D60, CMD_A, dir.path()).commit(&staged).unwrap_err();
    assert!(matches!(err, UpdateError::Verify(_)), "{err}");
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
    assert!(!trial::record_path(dir.path()).exists());
    nand.flip_bit("root1", 3 * BLOCK as usize);
    // As staged: selected, with the trial recorded.
    let done = Updater::new(&mut m, Model::D60, CMD_A, dir.path()).commit(&staged).unwrap();
    assert!(matches!(done.target, Target::Bank { bank: Bank::B, previous: Bank::A, .. }), "{:?}", done.target);
    assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::B);
    assert_bank_holds(&nand, Bank::B, &new);
    assert_eq!(trial::load(dir.path()).unwrap().previous_bank, Bank::A);
}

/// `dphone update status`: a bank whose root file system is only partly
/// there (the incident: the first 28 blocks, erased beyond) has a valid
/// header and a loadable kernel, and is still not the image.
#[test]
fn verify_bank_compares_both_components_with_the_header() {
    let (kl, rl) = sizes();
    let a = image("D60", "2_9_28", 1, kl, rl);
    let b = image("D60", "3_0_0", 3, kl, rl);
    let nand = phone(Model::D60, &a, &b, Bank::B);
    let mut m = nand.clone();
    let (ops, before) = (nand.ops(), nand.snapshot());
    assert_eq!(verify_bank(&mut m, Bank::A, &a.header).unwrap(), Ok(()));
    assert_eq!(verify_bank(&mut m, Bank::B, &b.header).unwrap(), Ok(()));
    nand.load("root1", &b.rootfs[..2 * BLOCK as usize]);
    let why = verify_bank(&mut m, Bank::B, &b.header).unwrap().unwrap_err();
    assert!(why.contains("root file system"), "{why}");
    assert!(inspect(&mut m, CMD_A, None).unwrap().banks[1].bootable(), "boot2's own checks still pass");
    nand.load("root1", &b.rootfs);
    nand.flip_bit("k1", 5000);
    let why = verify_bank(&mut m, Bank::B, &b.header).unwrap().unwrap_err();
    assert!(why.contains("kernel"), "{why}");
    assert_eq!(verify_bank(&mut m, Bank::A, &a.header).unwrap(), Ok(()));
    assert_eq!(nand.ops(), ops, "verifying never writes");
    nand.flip_bit("k1", 5000);
    assert_eq!(nand.snapshot(), before);
}

// ------------------------------------- shared with the early-boot stage

#[test]
fn the_shared_record_code_agrees_with_the_typed_record() {
    use opendeskphone_ctrl::record;
    for bytes in [unhex(D60_DUMP), unhex(D40_DUMP), unhex(D50_DUMP), d60_live()] {
        let rec = CtrlRecord::parse(&bytes).unwrap();
        assert!(record::valid(&bytes));
        assert_eq!(record::active(&bytes), rec.active);
        assert_eq!(record::boot_side(&bytes), rec.boot_side());
        let raw: record::Record = bytes.clone().try_into().unwrap();
        for active in [0, 1] {
            let mut want = rec;
            want.active = active;
            assert_eq!(record::with_active(&raw, active), want.to_bytes());
        }
        for (i, side) in [Side::INVALID, rec.sides[1]].into_iter().enumerate() {
            let mut one = rec;
            one.sides[0] = side;
            let b = one.to_bytes();
            assert_eq!(record::side_valid(&b, 0), i == 1);
            assert!(record::side_valid(&b, 1));
            assert_eq!(record::boot_side(&b), one.boot_side());
        }
        let mut bad = bytes.clone();
        bad[3] ^= 1;
        assert!(!record::valid(&bad));
    }
    assert!(!record::valid(&[0xff; 304]));
    assert!(!record::valid(&[0u8; 304]));
    assert!(!record::valid(&[0u8; 100]));
    // The layout the early stage takes from `digium_board=` is the family's.
    for model in Model::ALL {
        let cmdline = format!("console=ttyAMA0 digium_board={model} root=/dev/mtdblock6 ro");
        assert_eq!(opendeskphone_ctrl::boot::replication(&cmdline), repl_of(model), "{model}");
    }
}

/// An MTD partition as the early stage's writer sees it.
struct Raw(Box<dyn opendeskphone_hal::MtdPartition>);

impl opendeskphone_ctrl::Flash for Raw {
    type Error = opendeskphone_hal::HalError;
    fn geometry(&self) -> opendeskphone_ctrl::Geometry {
        let i = self.0.info();
        opendeskphone_ctrl::Geometry { size: i.size, erase_size: i.erase_size, write_size: i.write_size }
    }
    fn is_bad(&mut self, offset: u64) -> Result<bool, Self::Error> {
        self.0.is_bad(offset)
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(offset, buf)
    }
    fn erase(&mut self, offset: u64) -> Result<(), Self::Error> {
        self.0.erase(offset)
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), Self::Error> {
        self.0.write(offset, data)
    }
}

/// What the early stage does when a trial is used up or the new root is
/// unusable (`opendeskphone-early`, `go_back`), on the same flash as the
/// updater: the result is byte for byte what the updater's own bank switch
/// writes.
#[test]
fn the_early_stage_goes_back_exactly_as_the_updater_would() {
    use opendeskphone_ctrl::{boot, record, trial::Trial};
    let (kl, rl) = sizes();
    for model in [Model::D60, Model::D40] {
        let name = model.to_string();
        let old = image(&name, "2_9_28", 1, kl, rl);
        let new = image(&name, "3_0_0", 3, kl, rl);
        let nand = phone(model, &old, &old, Bank::A);
        let dir = tempfile::tempdir().unwrap();
        install_in(&nand, model, CMD_A, &new.eff, dir.path()).unwrap();
        let mut m = nand.clone();
        assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::B);

        // The record the updater left is the early stage's to count: three
        // boots of bank B, and the fourth goes back.
        let text = std::fs::read(trial::record_path(dir.path())).unwrap();
        let mut trial = Trial::parse(&text).unwrap();
        for _ in 0..3 {
            let boot::Step::Count(next) = boot::step(Some(1), Some(trial)) else { panic!("not counted") };
            trial = next;
        }
        let boot::Step::GoBack(trial) = boot::step(Some(1), Some(trial)) else { panic!("did not go back") };
        assert_eq!(trial.previous_bank, 0);
        // What the updater's parser makes of the record as the early stage
        // stores it.
        let mut buf = [0u8; opendeskphone_ctrl::trial::TEXT_MAX];
        let n = trial.format(&mut buf);
        let typed = trial::TrialRecord::parse(std::str::from_utf8(&buf[..n]).unwrap()).unwrap();
        assert_eq!((typed.previous_bank, typed.trial_bank, typed.boots), (Bank::A, Some(Bank::B), 4));

        // The updater's way back, on a copy.
        let reference = nand.clone_contents();
        select_bank(&mut reference.clone(), model, Bank::A).unwrap();

        // The early stage's.
        let cmdline = format!("digium_board={name} root=/dev/mtdblock7 rootfstype=jffs2 ro");
        let mut part = Raw(m.open("ctrl").unwrap());
        let current = opendeskphone_ctrl::scan(&mut part).unwrap().record.unwrap();
        assert!(record::side_valid(&current, trial.previous_bank as usize));
        let wanted = record::with_active(&current, trial.previous_bank as u32);
        let mut scratch = vec![0u8; 2 * BLOCK as usize];
        assert!(matches!(opendeskphone_ctrl::write(&mut part, &wanted, boot::replication(&cmdline), &mut scratch), Ok(0)));
        assert_eq!(nand.contents("ctrl"), reference.contents("ctrl"), "{model}");
        assert_eq!(boot2::simulate(&mut m).unwrap().side, Bank::A);
        assert_bank_holds(&nand, Bank::A, &old);
        // Too little scratch space is refused before anything is erased.
        let ops = nand.ops();
        let other = record::with_active(&current, 1);
        let e = opendeskphone_ctrl::write(&mut part, &other, boot::replication(&cmdline), &mut scratch[..BLOCK as usize]);
        assert!(matches!(e, Err(opendeskphone_ctrl::WriteError::Geometry)));
        assert_eq!(nand.ops(), ops);
    }
}
