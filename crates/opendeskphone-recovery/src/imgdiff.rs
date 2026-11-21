//! The stock recovery image, made from a stock package.
//!
//! A stock package has no recovery image.  It has the boot image, and the
//! difference between the two (`recovery/recovery-from-boot.p`, in the
//! format of Android's `imgdiff`), which the stock system applies when it
//! starts, with `system/etc/recovery-resource.dat` for the recovery's
//! pictures.  [`stock_recovery`] applies it the same way, so that `dphone`
//! can write the stock recovery before it leaves a stock package for it.
//!
//! An `imgdiff` patch takes the image in chunks.  A chunk of plain bytes
//! is a `bsdiff` patch of a part of the source.  A chunk of deflated data
//! (the ramdisk, which is a gzip file) is a `bsdiff` patch of the
//! *unpacked* source data, and the result is deflated again with zlib's
//! settings, which the patch names: Android relies on zlib giving the very
//! bytes the image was built with.
//!
//! There is no zlib here, and another compressor gives other bytes for the
//! same data.  So the ramdisk of the image made here holds the same files
//! as the stock one, compressed differently, and the image's header gives
//! its length and checksum as they then are.  The image is not the stock
//! one byte for byte: the stock system, once it runs, finds that out (it
//! compares a SHA-1) and writes its own over it.

use sha1::{Digest, Sha1};
use std::io::Read;
use std::ops::Range;

/// The page of the D80's boot images.
const PAGE: usize = 2048;
/// Where a boot image's header has the SHA-1 of its parts.
const ID_OFFSET: usize = 576;

fn le32(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize)
}

fn le64(data: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?)).ok()
}

/// A number as `bsdiff` writes it: eight bytes, the sign in the top bit.
fn offtin(data: &[u8], at: usize) -> Option<i64> {
    let raw = u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?);
    let magnitude = (raw & !(1 << 63)) as i64;
    Some(if raw >> 63 == 1 { -magnitude } else { magnitude })
}

fn bunzip(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    bzip2_rs::DecoderReader::new(data)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("the patch's compressed data is damaged: {e}"))?;
    if out.len() > limit {
        return Err("the patch unpacks to more than it may".into());
    }
    Ok(out)
}

/// Applies a `bsdiff` patch (`BSDIFF40`) to `old`.
pub fn bspatch(old: &[u8], patch: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let damaged = || "the patch is damaged".to_string();
    if patch.get(..8) != Some(b"BSDIFF40") {
        return Err("not a bsdiff patch".into());
    }
    let ctrl_len = offtin(patch, 8).and_then(|n| usize::try_from(n).ok()).ok_or_else(damaged)?;
    let diff_len = offtin(patch, 16).and_then(|n| usize::try_from(n).ok()).ok_or_else(damaged)?;
    let new_len = offtin(patch, 24).and_then(|n| usize::try_from(n).ok()).ok_or_else(damaged)?;
    if new_len > limit {
        return Err("the patch makes more than it may".into());
    }
    let ctrl_end = 32usize.checked_add(ctrl_len).ok_or_else(damaged)?;
    let diff_end = ctrl_end.checked_add(diff_len).ok_or_else(damaged)?;
    let ctrl = bunzip(patch.get(32..ctrl_end).ok_or_else(damaged)?, 24 * (new_len + 1))?;
    let diff = bunzip(patch.get(ctrl_end..diff_end).ok_or_else(damaged)?, new_len)?;
    let extra = bunzip(patch.get(diff_end..).ok_or_else(damaged)?, new_len)?;

    let mut new = Vec::with_capacity(new_len);
    let (mut old_at, mut diff_at, mut extra_at, mut ctrl_at) = (0i64, 0usize, 0usize, 0usize);
    while new.len() < new_len {
        let add = offtin(&ctrl, ctrl_at).and_then(|n| usize::try_from(n).ok()).ok_or_else(damaged)?;
        let copy = offtin(&ctrl, ctrl_at + 8).and_then(|n| usize::try_from(n).ok()).ok_or_else(damaged)?;
        let seek = offtin(&ctrl, ctrl_at + 16).ok_or_else(damaged)?;
        ctrl_at += 24;
        if add > new_len - new.len() || copy > new_len - new.len() - add {
            return Err(damaged());
        }
        let added = diff.get(diff_at..diff_at + add).ok_or_else(damaged)?;
        for (i, d) in added.iter().enumerate() {
            let from = usize::try_from(old_at + i as i64).ok().and_then(|at| old.get(at)).copied().unwrap_or(0);
            new.push(d.wrapping_add(from));
        }
        diff_at += add;
        old_at += add as i64;
        new.extend_from_slice(extra.get(extra_at..extra_at + copy).ok_or_else(damaged)?);
        extra_at += copy;
        old_at += seek;
    }
    Ok(new)
}

/// A deflated chunk of the result: where it is, and the CRC-32 and the
/// length of what it unpacks to.
struct Deflated {
    at: Range<usize>,
    crc: u32,
    len: usize,
}

/// Applies an `imgdiff` patch (`IMGDIFF2`).  `bonus` is data the patch
/// takes to follow the unpacked source of its deflated chunk.
fn apply(source: &[u8], patch: &[u8], bonus: &[u8], limit: usize) -> Result<(Vec<u8>, Vec<Deflated>), String> {
    let damaged = || "the recovery patch is damaged".to_string();
    if patch.get(..8) != Some(b"IMGDIFF2") {
        return Err("the recovery patch is not an imgdiff patch".into());
    }
    let chunks = le32(patch, 8).ok_or_else(damaged)?;
    let mut at = 12;
    let (mut out, mut deflated) = (Vec::new(), Vec::new());
    for _ in 0..chunks {
        let kind = le32(patch, at).ok_or_else(damaged)?;
        at += 4;
        let left = limit.saturating_sub(out.len());
        match kind {
            // Plain bytes.
            0 => {
                let (start, len, offset) =
                    (le64(patch, at).ok_or_else(damaged)?, le64(patch, at + 8).ok_or_else(damaged)?, le64(patch, at + 16).ok_or_else(damaged)?);
                at += 24;
                let old = start.checked_add(len).and_then(|end| source.get(start..end)).ok_or_else(damaged)?;
                out.extend_from_slice(&bspatch(old, patch.get(offset..).ok_or_else(damaged)?, left)?);
            }
            // Deflated data.
            2 => {
                let word = |i: usize| le64(patch, at + 8 * i).ok_or_else(damaged);
                let (start, len, offset, old_len, new_len) = (word(0)?, word(1)?, word(2)?, word(3)?, word(4)?);
                let level = le32(patch, at + 40).ok_or_else(damaged)?;
                at += 60;
                if old_len > limit * 8 || new_len > limit * 8 {
                    return Err(damaged());
                }
                let packed = start.checked_add(len).and_then(|end| source.get(start..end)).ok_or_else(damaged)?;
                let mut old = miniz_oxide::inflate::decompress_to_vec_with_limit(packed, old_len)
                    .map_err(|_| "the boot image's ramdisk does not unpack".to_string())?;
                old.extend_from_slice(bonus);
                if old.len() != old_len {
                    return Err("the recovery patch is not for this boot image and these resources".into());
                }
                let new = bspatch(&old, patch.get(offset..).ok_or_else(damaged)?, new_len)?;
                if new.len() != new_len {
                    return Err(damaged());
                }
                let packed = miniz_oxide::deflate::compress_to_vec(&new, level.min(10) as u8);
                deflated.push(Deflated { at: out.len()..out.len() + packed.len(), crc: !eff::crc::crc(&new), len: new.len() });
                out.extend_from_slice(&packed);
            }
            // Bytes given as they are.
            3 => {
                let len = le32(patch, at).ok_or_else(damaged)?;
                out.extend_from_slice(patch.get(at + 4..at + 4 + len).ok_or_else(damaged)?);
                at += 4 + len;
            }
            other => return Err(format!("the recovery patch has a chunk of a kind ({other}) that is not known here")),
        }
        if out.len() > limit {
            return Err("the recovery patch makes more than the recovery partition holds".into());
        }
    }
    Ok((out, deflated))
}

/// The stock recovery image: the package's `boot.img` with its
/// `recovery/recovery-from-boot.p` applied, with
/// `system/etc/recovery-resource.dat` as `resources`.  It is no longer
/// than `limit`, the size of the recovery partition.
pub fn stock_recovery(boot: &[u8], patch: &[u8], resources: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let odd = || "the stock recovery image is not laid out as expected".to_string();
    let (mut image, deflated) = apply(boot, patch, resources, limit)?;
    if image.get(..8) != Some(b"ANDROID!") || le32(&image, 36) != Some(PAGE) {
        return Err("the recovery patch does not make a boot image".into());
    }
    // The one deflated chunk is the body of the ramdisk's gzip file: after
    // the kernel's pages, a header without optional fields, the deflated
    // data, and the CRC-32 and the length of what it unpacks to.
    let [ramdisk] = &deflated[..] else { return Err(odd()) };
    let kernel_len = le32(&image, 8).ok_or_else(odd)?;
    let start = PAGE + kernel_len.div_ceil(PAGE) * PAGE;
    if image.get(start..start + 4) != Some(&[0x1f, 0x8b, 0x08, 0x00]) || ramdisk.at.start != start + 10 {
        return Err(odd());
    }
    let end = ramdisk.at.end + 8;
    let trailer = image.get(ramdisk.at.end..end).ok_or_else(odd)?;
    if trailer[..4] != ramdisk.crc.to_le_bytes() || trailer[4..] != (ramdisk.len as u32).to_le_bytes() {
        return Err("the stock recovery's ramdisk is not what its checksum says: the patch is not for this boot image".into());
    }
    // The ramdisk's length, and the SHA-1 of the kernel, its length, the
    // ramdisk, its length, and the length of the second-stage image (none).
    image.truncate(end);
    image[16..20].copy_from_slice(&((end - start) as u32).to_le_bytes());
    let mut id = Sha1::new();
    for part in [&image[PAGE..PAGE + kernel_len], &image[start..end], &[]] {
        id.update(part);
        id.update((part.len() as u32).to_le_bytes());
    }
    let id = id.finalize();
    image[ID_OFFSET..ID_OFFSET + 20].copy_from_slice(&id);
    image.resize(end.div_ceil(PAGE) * PAGE, 0);
    if image.len() > limit {
        return Err(format!("the stock recovery image is {} bytes; its partition holds {limit}", image.len()));
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT: &[u8] = include_bytes!("../testdata/boot.img");
    const PATCH: &[u8] = include_bytes!("../testdata/recovery-from-boot.p");
    const RESOURCES: &[u8] = include_bytes!("../testdata/recovery-resource.dat");
    const RAMDISK: &[u8] = include_bytes!("../testdata/recovery-ramdisk");

    #[test]
    fn a_stock_package_makes_its_recovery_image() {
        let image = stock_recovery(BOOT, PATCH, RESOURCES, 8 << 20).unwrap();
        assert_eq!(image.len() % PAGE, 0);
        // The kernel is the boot image's, and the ramdisk unpacks to the
        // recovery's.
        let kernel_len = le32(&image, 8).unwrap();
        assert_eq!(kernel_len, 768);
        assert_eq!(image[PAGE..PAGE + kernel_len], BOOT[PAGE..PAGE + kernel_len]);
        let (start, len) = (2 * PAGE, le32(&image, 16).unwrap());
        let gzip = &image[start..start + len];
        assert_eq!(gzip[..4], [0x1f, 0x8b, 0x08, 0x00]);
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(&gzip[10..len - 8]).unwrap(), RAMDISK);
        assert_eq!(gzip[len - 8..len - 4], (!eff::crc::crc(RAMDISK)).to_le_bytes());
        assert_eq!(gzip[len - 4..], (RAMDISK.len() as u32).to_le_bytes());
        assert!(image[start + len..].iter().all(|b| *b == 0));
        // The header's checksum is of the parts as they are here.
        let mut id = Sha1::new();
        for part in [&image[PAGE..PAGE + kernel_len], gzip, &[]] {
            id.update(part);
            id.update((part.len() as u32).to_le_bytes());
        }
        assert_eq!(image[ID_OFFSET..ID_OFFSET + 20], id.finalize()[..]);
    }

    #[test]
    fn a_patch_for_other_files_is_refused() {
        assert!(stock_recovery(BOOT, PATCH, &RESOURCES[1..], 8 << 20).unwrap_err().contains("not for this boot image"));
        let mut other = BOOT.to_vec();
        other[2 * PAGE + 12] ^= 0x55;
        assert!(stock_recovery(&other, PATCH, RESOURCES, 8 << 20).is_err());
        assert!(stock_recovery(BOOT, &PATCH[..PATCH.len() - 10], RESOURCES, 8 << 20).is_err());
        assert!(stock_recovery(BOOT, b"IMGDIFF1", RESOURCES, 8 << 20).is_err());
        // More than the partition holds.
        assert!(stock_recovery(BOOT, PATCH, RESOURCES, 4096).is_err());
    }
}
