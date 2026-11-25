//! Firmware images (`.eff` files): what one says it is, and what
//! identifies it.
//!
//! An `.eff` file is a 100-byte plaintext label (`<model> <version>`), then
//! OpenSSL "Salted__" AES-256-CBC ciphertext (key and IV from
//! `EVP_BytesToKey` with MD5, one round, over the passphrase that the
//! model's phones hold) of four 100-byte text fields followed by the update
//! itself.  A D40 thru D70's update starts with a 160-byte header that
//! carries the CRC and size of its two components, the kernel and the root
//! file system.  A D80's update is an Android OTA package, a zip archive;
//! Open Desk Phone's states the CRC and size of its boot image and root
//! file system image in its first file, the metadata, as
//! `opendeskphone-checksum=` and the text of [`ImageChecksum`].
//!
//! Those four numbers identify the image: a phone records them for the
//! image it runs, and a server states them in each `<firmware>` entry, so
//! that the phone sees whether it already runs the image without fetching
//! it.  [`Image::read`] reads them, for the phone and the server alike.

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use md5::{Digest, Md5};
use std::io::Read;

/// The passphrase of the D40 thru D70's images (the phones'
/// `/etc/otapassphrase`).
const PASSPHRASE: &[u8; 255] = b"*\xac7\xf5\x9e1\xb0\xde\xef\xfb1\xf4N`V#\x82\xa1%\xb6$)\xd9\x12\xc3\xb3\xc4\x89\xbd1O\xb4\xac|\xcf\xb0\"k\x92\xf5\x14K'T\x8e \x8c\xe2\x89\xe9\x03#>\x84\x0e0\x11,~\xd8\xdc#3~}\xb9m&\xad\x18|~J#G)j\x17\x8e\xe0\x07\xa3\xb7k\xa7\xa6BF0@?MEv\xad\xa24\xc4 \x06-t\xf2\xb8q\xf8\xb9\x8d \x16\x1a\xa5}\x18\xdc\x03)\xc3\xb0\x03\x03]q\x0f\xbaI<\xf3\xf4Q\xb2\xfb\xe3\xf1\x8e(\xc5\xa2\xfdX\xf8\xb1\xb7\x17\x9cL \xca0\xf0:\x07\xd80\xb9\x97\x06\x17\x89\x01\x03v\xc5\x1c\xc0\xb2\xed-\xff\xd9\x15_+\xe1}\x9d\x95&~\xe4\xdc\xcd\x061Uf\x90 VR\xa0 \xd6\xcf\xec\x1eP\xcf\xdaU\xe0\xa1\x14\xe7t\\N\x06\x1b\xab\xf5\xe7\x1f\x85\x9f\xd05\xa3\xd1QY\xe1\x10\xe9\xcc\xdf_d\x87\xcf\xa4\x9c5$\xbdq_\xdb\xee\x1a\x07^\xeb\xc7G\x90\xdc{\xa1\x88\xef=\xb6Wq";
/// The passphrase of the D80's images.
const D80_PASSPHRASE: &[u8; 47] = b"q0\xcev\xf5\"\x16\xd15N7\xee\xbb\x0ez\xec\x8d\xf3\x82\xb5\xc0z\x9b\xb2>_8u\x9d\x12\xa6\xc3%R\xde\xddz/\x84j0\x8d)0\xf4\xde\xc8";

const LABEL_LEN: usize = 100;
const INNER_LEN: usize = 400;
const UPDATE_HEADER_LEN: usize = 160;
/// How much more of a D80's update is read for the package's metadata.
const D80_MORE: usize = 512;
const D80_METADATA: &[u8] = b"META-INF/com/android/metadata";
const D80_CHECKSUM_KEY: &str = "opendeskphone-checksum=";
const MAGIC_TEXT: &[u8] = b"Digium Phone Firmware\0";
const UPDATE_MAGIC: u32 = 0x7d9f_8001;

/// The CRC of the phones' updater: reflected CRC-32 (polynomial
/// 0xEDB88320) from 0xffffffff, without the final inversion.
fn crc(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    c
}

fn be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le16(b: &[u8]) -> usize {
    usize::from(u16::from_le_bytes([b[0], b[1]]))
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The checksums and sizes that identify a firmware image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageChecksum {
    pub kernel_crc: u32,
    pub kernel_size: u32,
    pub rootfs_crc: u32,
    pub rootfs_size: u32,
}

/// What the start of an `.eff` file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The model in the encrypted header, e.g., `D60`.
    pub model: String,
    /// The version in the encrypted header, e.g., `3_1_0`.
    pub version: String,
    pub checksum: ImageChecksum,
}

impl Image {
    /// Reads the start of an `.eff` file: its first 676 bytes, and 512 more
    /// of a D80's.  The components themselves are not read.
    pub fn read(mut file: impl Read) -> Result<Image, String> {
        let cut_short = |e: std::io::Error| format!("firmware image is cut short: {e}");
        let mut head = [0u8; LABEL_LEN + 16];
        file.read_exact(&mut head).map_err(cut_short)?;
        if &head[LABEL_LEN..LABEL_LEN + 8] != b"Salted__" {
            return Err("not a firmware image".into());
        }
        let label_model = head[..LABEL_LEN]
            .split(|&b| b == b' ' || b == 0)
            .next()
            .unwrap_or_default();
        let passphrase: &[u8] = if label_model.eq_ignore_ascii_case(b"D80") {
            D80_PASSPHRASE
        } else {
            PASSPHRASE
        };
        let (key, iv) = derive(passphrase, &head[LABEL_LEN + 8..]);

        let mut buf = [0u8; INNER_LEN + UPDATE_HEADER_LEN];
        file.read_exact(&mut buf).map_err(cut_short)?;
        // The IV of the ciphertext that follows.
        let mut next_iv = [0u8; 16];
        next_iv.copy_from_slice(&buf[buf.len() - 16..]);
        decrypt(&key, &iv, &mut buf);
        if !buf.starts_with(MAGIC_TEXT) {
            return Err("not a firmware image (its header does not decrypt)".into());
        }
        let field = |b: &[u8]| {
            let text = b.split(|&c| c == 0).next().unwrap_or_default();
            String::from_utf8_lossy(text).trim().to_string()
        };
        let (model, version) = (field(&buf[200..300]), field(&buf[300..400]));
        let update = &buf[INNER_LEN..];
        let checksum = if update.starts_with(b"PK\x03\x04") {
            let mut more = [0u8; D80_MORE];
            file.read_exact(&mut more).map_err(cut_short)?;
            decrypt(&key, &next_iv, &mut more);
            package_checksum(&[update, &more[..]].concat())?
        } else {
            update_checksum(update)?
        };
        Ok(Image {
            model,
            version,
            checksum,
        })
    }
}

/// `EVP_BytesToKey(MD5, one round)`: 32 bytes of key, 16 of IV.
fn derive(passphrase: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 16]) {
    let mut derived = [0u8; 48];
    let mut prev: Option<[u8; 16]> = None;
    for chunk in derived.chunks_mut(16) {
        let mut h = Md5::new();
        if let Some(p) = prev {
            h.update(p);
        }
        h.update(passphrase);
        h.update(salt);
        let d: [u8; 16] = h.finalize().into();
        chunk.copy_from_slice(&d);
        prev = Some(d);
    }
    let (mut key, mut iv) = ([0u8; 32], [0u8; 16]);
    key.copy_from_slice(&derived[..32]);
    iv.copy_from_slice(&derived[32..]);
    (key, iv)
}

/// Decrypts whole blocks in place.
fn decrypt(key: &[u8; 32], iv: &[u8; 16], data: &mut [u8]) {
    cbc::Decryptor::<aes::Aes256>::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(data)
        .expect("whole blocks");
}

/// The checksum in a D40 thru D70's update header: magic, type (0: kernel
/// and root file system), the CRC of those two, then the component header
/// and its CRC.  The CRCs are over the integers in little-endian order.
fn update_checksum(u: &[u8]) -> Result<ImageChecksum, String> {
    if be(&u[0..4]) != UPDATE_MAGIC || be(&u[4..8]) != 0 {
        return Err("not a firmware image of a kernel and a root file system".into());
    }
    let sum = ImageChecksum {
        kernel_crc: be(&u[12..16]),
        kernel_size: be(&u[16..20]),
        rootfs_crc: be(&u[20..24]),
        rootfs_size: be(&u[24..28]),
    };
    let mut covered = Vec::with_capacity(144);
    for v in [
        sum.kernel_crc,
        sum.kernel_size,
        sum.rootfs_crc,
        sum.rootfs_size,
    ] {
        covered.extend_from_slice(&v.to_le_bytes());
    }
    covered.extend_from_slice(&u[28..156]);
    if crc(&covered) != be(&u[156..160]) {
        return Err("the firmware image's update header is damaged".into());
    }
    Ok(sum)
}

/// The checksum that a D80's package states in its metadata, the package's
/// first file, with which `package` starts.  A package that states none,
/// as stock firmware's, is named by the CRC-32 and the length of its
/// metadata, with zeros for the root file system, which no Open Desk Phone
/// image has.
fn package_checksum(package: &[u8]) -> Result<ImageChecksum, String> {
    let damaged = || "the firmware package's metadata is damaged".to_string();
    let header = package.get(..30).ok_or_else(damaged)?;
    let (flags, method) = (le16(&header[6..]), le16(&header[8..]));
    let (crc32, size) = (le32(&header[14..]), le32(&header[22..]) as usize);
    let (name_len, extra) = (le16(&header[26..]), le16(&header[28..]));
    if package.get(30..30 + name_len) != Some(D80_METADATA) {
        return Err("the firmware package does not start with its metadata".into());
    }
    let data = package.get(30 + name_len + extra..).ok_or_else(damaged)?;
    // With bit 3, the CRC and the sizes follow the data, as a writer that
    // streams has them.
    let described_after = flags & 8 != 0;
    let metadata = match method {
        0 if !described_after => data.get(..size).ok_or_else(damaged)?.to_vec(),
        // Deflated data ends with its last block: what follows is not
        // taken for part of it.
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, D80_MORE)
            .map_err(|_| damaged())?,
        _ => {
            return Err(format!(
                "the firmware package's metadata is stored in a way (method {method}, flags {flags:#x}) that is not read here"
            ))
        }
    };
    if !described_after && (metadata.len() != size || !crc(&metadata) != crc32) {
        return Err(damaged());
    }
    let text = String::from_utf8_lossy(&metadata);
    match text
        .lines()
        .find_map(|line| line.strip_prefix(D80_CHECKSUM_KEY))
    {
        Some(stated) => stated
            .parse()
            .map_err(|()| format!("the firmware package's checksum {stated} is not one")),
        None => Ok(ImageChecksum {
            kernel_crc: !crc(&metadata),
            kernel_size: metadata.len() as u32,
            rootfs_crc: 0,
            rootfs_size: 0,
        }),
    }
}

/// The text form, as [`Display`](std::fmt::Display) writes it.
impl std::str::FromStr for ImageChecksum {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let mut f = text.trim().split('-');
        let mut next = || f.next().ok_or(());
        let sum = ImageChecksum {
            kernel_crc: u32::from_str_radix(next()?, 16).map_err(|_| ())?,
            kernel_size: next()?.parse().map_err(|_| ())?,
            rootfs_crc: u32::from_str_radix(next()?, 16).map_err(|_| ())?,
            rootfs_size: next()?.parse().map_err(|_| ())?,
        };
        if f.next().is_some() {
            return Err(());
        }
        Ok(sum)
    }
}

/// The value of a `<firmware>` entry's `checksum` attribute:
/// `<kernel CRC>-<kernel size>-<rootfs CRC>-<rootfs size>`, the CRCs as
/// eight lowercase hexadecimal digits and the sizes in decimal bytes.
impl std::fmt::Display for ImageChecksum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:08x}-{}-{:08x}-{}",
            self.kernel_crc, self.kernel_size, self.rootfs_crc, self.rootfs_size
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;

    /// The start of an image for `model` whose update starts with `update`.
    fn image(model: &str, update: &[u8]) -> Vec<u8> {
        let salt = *b"saltsalt";
        let passphrase: &[u8] = if model == "D80" {
            D80_PASSPHRASE
        } else {
            PASSPHRASE
        };
        let (key, iv) = derive(passphrase, &salt);
        let mut plain = vec![0u8; INNER_LEN + update.len()];
        plain[..21].copy_from_slice(b"Digium Phone Firmware");
        plain[100] = b'1';
        plain[200..203].copy_from_slice(model.as_bytes());
        plain[300..306].copy_from_slice(b"9_9_99");
        plain[INNER_LEN..].copy_from_slice(update);
        let len = plain.len();
        cbc::Encryptor::<aes::Aes256>::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut plain, len)
            .unwrap();
        let mut out = vec![0u8; LABEL_LEN];
        out[..10].copy_from_slice(format!("{model} 9_9_99").as_bytes());
        out.extend_from_slice(b"Salted__");
        out.extend_from_slice(&salt);
        out.extend_from_slice(&plain);
        out
    }

    fn update_header(sum: ImageChecksum) -> [u8; UPDATE_HEADER_LEN] {
        let mut u = [0u8; UPDATE_HEADER_LEN];
        u[0..4].copy_from_slice(&UPDATE_MAGIC.to_be_bytes());
        u[12..16].copy_from_slice(&sum.kernel_crc.to_be_bytes());
        u[16..20].copy_from_slice(&sum.kernel_size.to_be_bytes());
        u[20..24].copy_from_slice(&sum.rootfs_crc.to_be_bytes());
        u[24..28].copy_from_slice(&sum.rootfs_size.to_be_bytes());
        u[28..52].copy_from_slice(b"Tue Oct 27 13:05:08 2026");
        u[92..108].copy_from_slice(b"9_9_99_D60_fw.bin"[..16].as_ref());
        let mut covered = Vec::new();
        for v in [
            sum.kernel_crc,
            sum.kernel_size,
            sum.rootfs_crc,
            sum.rootfs_size,
        ] {
            covered.extend_from_slice(&v.to_le_bytes());
        }
        covered.extend_from_slice(&u[28..156]);
        u[156..160].copy_from_slice(&crc(&covered).to_be_bytes());
        u
    }

    /// The start of a D80's package whose first file is `metadata`: stored,
    /// as Open Desk Phone writes it, or deflated with its CRC and sizes
    /// after it, as stock firmware's is.
    fn package(metadata: &[u8], deflated: bool) -> Vec<u8> {
        let (flags, method) = if deflated { (8u16, 8u16) } else { (0, 0) };
        let data = if deflated {
            miniz_oxide::deflate::compress_to_vec(metadata, 6)
        } else {
            metadata.to_vec()
        };
        let mut p = b"PK\x03\x04".to_vec();
        for w in [20u16, flags, method, 0, 33] {
            p.extend_from_slice(&w.to_le_bytes());
        }
        let len = metadata.len() as u32;
        let words = if deflated {
            [0, 0, 0]
        } else {
            [!crc(metadata), len, len]
        };
        for w in words {
            p.extend_from_slice(&w.to_le_bytes());
        }
        p.extend_from_slice(&(D80_METADATA.len() as u16).to_le_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(D80_METADATA);
        p.extend_from_slice(&data);
        p.resize(UPDATE_HEADER_LEN + D80_MORE, 0xee);
        p
    }

    const SUM: ImageChecksum = ImageChecksum {
        kernel_crc: 0x0a1b_2c3d,
        kernel_size: 1_500_000,
        rootfs_crc: 0xffff_ffff,
        rootfs_size: 39_321_600,
    };

    #[test]
    fn crc_is_the_updaters() {
        // CRC-32 of "123456789" is 0xcbf43926; without the final inversion
        // it is the complement.
        assert_eq!(crc(b"123456789"), !0xcbf4_3926);
    }

    #[test]
    fn reads_an_image_and_formats_the_attribute() {
        let read = Image::read(&image("D60", &update_header(SUM))[..]).unwrap();
        assert_eq!(
            read,
            Image {
                model: "D60".into(),
                version: "9_9_99".into(),
                checksum: SUM
            }
        );
        assert_eq!(SUM.to_string(), "0a1b2c3d-1500000-ffffffff-39321600");
        assert_eq!(SUM.to_string().parse(), Ok(SUM));
    }

    #[test]
    fn refuses_what_is_not_an_image() {
        let good = image("D60", &update_header(SUM));
        assert!(Image::read(&good[..good.len() - 1]).is_err(), "short");
        assert!(Image::read(&b"not firmware"[..]).is_err());
        let mut unsalted = good.clone();
        unsalted[100] = b'X';
        assert!(Image::read(&unsalted[..]).is_err());
        // Another passphrase's ciphertext does not decrypt to the magic.
        let mut other = good.clone();
        other[110] ^= 1;
        assert!(Image::read(&other[..]).is_err());
        // A damaged component header.
        let mut u = update_header(SUM);
        u[13] ^= 1;
        assert!(Image::read(&image("D60", &u)[..]).is_err());
        // A bootstrap update (type 1).
        let mut u = update_header(SUM);
        u[7] = 1;
        assert!(Image::read(&image("D60", &u)[..]).is_err());
    }

    #[test]
    fn a_d80_image_is_named_by_its_package_metadata() {
        let ours = b"post-build=OpenDeskPhone/model80/3_1_0\npre-device=model80\nopendeskphone-checksum=0a1b2c3d-1500000-ffffffff-39321600\n";
        let read = Image::read(&image("D80", &package(ours, false))[..]).unwrap();
        assert_eq!((read.model.as_str(), read.checksum), ("D80", SUM));
        // Stock firmware's states no checksum, and is named so that it is
        // never taken for one of ours.
        let stock = b"post-build=Digium/digium_model80/model80:4.4.2/1_12_14/1_12_14:eng/dev-keys\npre-device=model80\n";
        let sum = Image::read(&image("D80", &package(stock, true))[..])
            .unwrap()
            .checksum;
        assert_eq!((sum.kernel_size, sum.rootfs_size), (stock.len() as u32, 0));
        let mut damaged = package(ours, false);
        damaged[70] ^= 1;
        assert!(Image::read(&image("D80", &damaged)[..]).is_err());
    }

    /// With `DPMA_TEST_EFF` naming a real image and `DPMA_TEST_EFF_SUM` its
    /// expected attribute, check a real file.
    #[test]
    fn reads_a_real_image_when_given_one() {
        let (Ok(path), Ok(want)) = (
            std::env::var("DPMA_TEST_EFF"),
            std::env::var("DPMA_TEST_EFF_SUM"),
        ) else {
            return;
        };
        let image = Image::read(std::fs::File::open(path).unwrap()).unwrap();
        assert_eq!(image.checksum.to_string(), want);
    }
}
