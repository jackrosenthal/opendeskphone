//! The whole-file signature of an OTA package, checked the way the stock
//! recovery (Android 4.4's) checks it.
//!
//! A signed package is a zip archive whose comment ends with the signature
//! and six bytes: the distance from the end of the file back to the start
//! of the signature block, `ff ff`, and the comment's length.  What is
//! signed is the file up to the comment's length field.  The signature is
//! RSA, PKCS #1 v1.5, over the SHA-1 of that, and it is the last bytes of
//! the block: the recovery takes as many bytes before the last six as the
//! key is long, and does not read the rest of the block.

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use std::io::{Read, Seek, SeekFrom};

/// The certificate of the key that the image build signs a package with.
/// The key is in the repository, so the signature does not show who made a
/// package; see `crates/opendeskphone-image/keys/README.md`.
pub const CERTIFICATE: &str = include_str!("../../opendeskphone-image/keys/d80-ota.x509.pem");

/// The six bytes at the end, and the directory's end record before the
/// comment.
const FOOTER: u64 = 6;
const EOCD: u64 = 22;
const EOCD_MARK: &[u8; 4] = b"PK\x05\x06";

/// The public key of an X.509 certificate (PEM).
pub fn certificate_key(pem: &str) -> Result<RsaPublicKey, String> {
    let (label, der) = rsa::pkcs8::Document::from_pem(pem).map_err(|e| format!("the certificate: {e}"))?;
    let cert = crate::der::Certificate::parse(der.as_bytes())
        .filter(|_| label == "CERTIFICATE")
        .ok_or("not an X.509 certificate")?;
    RsaPublicKey::from_public_key_der(cert.key).map_err(|e| format!("the certificate's key: {e}"))
}

/// The keys this firmware's recovery installs packages of: the one key in
/// the repository.
pub fn keys() -> Vec<RsaPublicKey> {
    vec![certificate_key(CERTIFICATE).expect("the certificate in the repository")]
}

/// Whether the `len` bytes of `file` end the way a signed package does:
/// the mark in the last six bytes, and the directory's end record where
/// the comment's length puts it.
pub fn has_footer<R: Read + Seek>(file: &mut R, len: u64) -> bool {
    layout(file, len).is_ok()
}

/// The length of what is signed, and the comment.
fn layout<R: Read + Seek>(file: &mut R, len: u64) -> Result<(u64, Vec<u8>), String> {
    let io = |e: std::io::Error| format!("reading the package: {e}");
    if len < FOOTER + EOCD {
        return Err("the package is too short to be signed".into());
    }
    let mut footer = [0u8; FOOTER as usize];
    file.seek(SeekFrom::Start(len - FOOTER)).map_err(io)?;
    file.read_exact(&mut footer).map_err(io)?;
    if footer[2..4] != [0xff, 0xff] {
        return Err("the package is not signed (it does not end with a signature's mark)".into());
    }
    let comment_len = u64::from(u16::from_le_bytes([footer[4], footer[5]]));
    let signature_start = u64::from(u16::from_le_bytes([footer[0], footer[1]]));
    if signature_start > comment_len || signature_start < FOOTER || len < comment_len + EOCD {
        return Err("the package's signature does not fit its comment".into());
    }
    let mut eocd = vec![0u8; (EOCD + comment_len) as usize];
    file.seek(SeekFrom::Start(len - EOCD - comment_len)).map_err(io)?;
    file.read_exact(&mut eocd).map_err(io)?;
    if &eocd[..4] != EOCD_MARK {
        return Err("the package's comment is not where its signature says".into());
    }
    // A comment with the look of another end record could hide files from
    // the signature.
    if eocd[4..].windows(4).any(|w| w == EOCD_MARK) {
        return Err("the package's comment has the mark of a directory's end in it".into());
    }
    let comment = eocd.split_off(EOCD as usize);
    Ok((len - comment_len - 2, comment))
}

/// Checks the signature of the package in the first `len` bytes of `file`
/// against `keys`, and returns the index of the key that made it.
/// `progress` is called with the part of the package read so far, 0 to 1.
pub fn verify<R: Read + Seek>(file: &mut R, len: u64, keys: &[RsaPublicKey], mut progress: impl FnMut(f32)) -> Result<usize, String> {
    let io = |e: std::io::Error| format!("reading the package: {e}");
    let (signed, comment) = layout(file, len)?;
    let block = &comment[..comment.len() - FOOTER as usize];

    let mut hash = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut left = signed;
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    while left > 0 {
        let n = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        file.read_exact(&mut buf[..n]).map_err(io)?;
        hash.update(&buf[..n]);
        left -= n as u64;
        progress((signed - left) as f32 / signed as f32);
    }
    let digest = hash.finalize();
    for (i, key) in keys.iter().enumerate() {
        let Some(signature) = block.len().checked_sub(key.size()).map(|at| &block[at..]) else { continue };
        if key.verify(Pkcs1v15Sign::new::<Sha1>(), &digest, signature).is_ok() {
            return Ok(i);
        }
    }
    Err("the package's signature is not by a key this recovery accepts".into())
}

/// What Android's `signapk` starts the comment of a signed package with.
const SIGNED_BY: &[u8] = b"signed by SignApk\0";

/// One DER element: its tag, and the parts of its content.
fn write(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = vec![tag];
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let bytes = &bytes[bytes.iter().take_while(|b| **b == 0).count()..];
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

/// Sign an OTA package the way `signapk -w` signs the whole file, which is
/// the signature a recovery checks.  `zip` is an archive without a comment;
/// `key` is an RSA private key (PKCS #8, PEM), and `cert` its X.509
/// certificate (PEM).
///
/// The signature goes in the archive's comment: the text `signed by
/// SignApk`, a zero, a PKCS #7 `SignedData` with the certificate, and six
/// bytes: the distance from the end of the file back to the start of the
/// `SignedData`, `ff ff`, and the comment's length.  What is signed is the
/// file up to the comment's length field.  The signature is PKCS #1 v1.5
/// over SHA-1, as the stock package's is, with no signed attributes, so
/// that it is the last bytes of the `SignedData`: an old recovery (the
/// stock one is Android 4.4's) does not parse the `SignedData`, and takes
/// the bytes before the last six for the signature.
///
/// The same input makes the same package: the signature has nothing random
/// in it.  The `META-INF/MANIFEST.MF`, `CERT.SF`, and `CERT.RSA` of a
/// signed jar, which `signapk` also adds and a recovery does not read, are
/// not added.
pub fn sign(zip: &[u8], key: &str, cert: &str) -> Result<Vec<u8>, String> {
    const SEQUENCE: u8 = 0x30;
    const SET: u8 = 0x31;
    const NULL: [u8; 2] = [0x05, 0x00];
    // The object identifiers, as DER elements.
    const SIGNED_DATA: &[u8] = b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x07\x02";
    const DATA: &[u8] = b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x07\x01";
    const SHA1: &[u8] = b"\x06\x05\x2b\x0e\x03\x02\x1a";
    const RSA: &[u8] = b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x01";
    const VERSION: &[u8] = b"\x02\x01\x01";

    let signed = zip.len().checked_sub(2).filter(|at| zip.len() >= 22 && zip[at - 20..at - 16] == *b"PK\x05\x06" && zip[*at..] == [0, 0]);
    let Some(signed) = signed.map(|at| &zip[..at]) else {
        return Err("the archive to sign does not end with a directory that has no comment".into());
    };

    let key = RsaPrivateKey::from_pkcs8_pem(key).map_err(|e| format!("the signing key: {e}"))?;
    let (label, cert) = rsa::pkcs8::Document::from_pem(cert).map_err(|e| format!("the signing certificate: {e}"))?;
    let cert = crate::der::Certificate::parse(cert.as_bytes()).filter(|_| label == "CERTIFICATE").ok_or("the signing certificate is not an X.509 certificate")?;

    let digest = Sha1::digest(signed);
    let signature = key.sign(Pkcs1v15Sign::new::<Sha1>(), &digest).map_err(|e| format!("signing: {e}"))?;
    let public = RsaPublicKey::from_public_key_der(cert.key).map_err(|e| format!("the signing certificate's key: {e}"))?;
    if public.verify(Pkcs1v15Sign::new::<Sha1>(), &digest, &signature).is_err() {
        return Err("the signing certificate is not the signing key's".into());
    }

    let sha1 = write(SEQUENCE, &[SHA1, &NULL]);
    let signer = write(
        SEQUENCE,
        &[
            VERSION,
            &write(SEQUENCE, &[cert.issuer, cert.serial]),
            &sha1,
            &write(SEQUENCE, &[RSA, &NULL]),
            &write(0x04, &[&signature]),
        ],
    );
    let signed_data = write(
        SEQUENCE,
        &[VERSION, &write(SET, &[&sha1]), &write(SEQUENCE, &[DATA]), &write(0xa0, &[cert.der]), &write(SET, &[&signer])],
    );
    let pkcs7 = write(SEQUENCE, &[SIGNED_DATA, &write(0xa0, &[&signed_data])]);

    // A recovery looks for the directory's end from the end of the file,
    // and refuses a comment that has the look of another one.
    if pkcs7.windows(4).any(|w| w == b"PK\x05\x06") {
        return Err("the signature has the mark of a zip directory's end in it".into());
    }
    let (Ok(from_end), Ok(comment)) = (u16::try_from(pkcs7.len() + 6), u16::try_from(SIGNED_BY.len() + pkcs7.len() + 6)) else {
        return Err("the signature is too large for a zip comment".into());
    };
    let mut out = signed.to_vec();
    out.extend_from_slice(&comment.to_le_bytes());
    out.extend_from_slice(SIGNED_BY);
    out.extend_from_slice(&pkcs7);
    out.extend_from_slice(&from_end.to_le_bytes());
    out.extend_from_slice(&[0xff, 0xff]);
    out.extend_from_slice(&comment.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = include_str!("../../opendeskphone-image/keys/d80-ota.key.pem");
    const CERT: &str = CERTIFICATE;

    /// A recovery's check of a package (Android's `verifier.cpp`), with
    /// the key in the tree.
    #[test]
    fn a_signed_package_is_what_a_recovery_checks() {
        let unsigned = crate::zip::write(&[crate::zip::FileEntry { name: "a", data: b"123456789", executable: false }]).unwrap();
        let z = sign(&unsigned, KEY, CERT).unwrap();
        assert_eq!(z, sign(&unsigned, KEY, CERT).unwrap());

        // The footer, and the end of the directory where it says.
        let footer = &z[z.len() - 6..];
        assert_eq!(footer[2..4], [0xff, 0xff]);
        let from_end = usize::from(u16::from_le_bytes([footer[0], footer[1]]));
        let comment = usize::from(u16::from_le_bytes([footer[4], footer[5]]));
        let end = z.len() - comment - 22;
        assert_eq!(&z[end..end + 4], b"PK\x05\x06");
        assert_eq!(z[end + 20..end + 22], footer[4..]);
        assert!(!z[end + 4..].windows(4).any(|w| w == b"PK\x05\x06"));
        assert!(z[end + 22..].starts_with(SIGNED_BY));
        // What is signed is the archive as it was, less the comment's length.
        let signed = &z[..z.len() - comment - 2];
        assert_eq!(signed, &unsigned[..unsigned.len() - 2]);

        // The signature is the last bytes before the footer, and the
        // PKCS #7 holds the certificate.
        let pkcs7 = &z[z.len() - from_end..z.len() - 6];
        assert_eq!(z[end + 22 + SIGNED_BY.len()..z.len() - 6], *pkcs7);
        assert_eq!(crate::der::element(pkcs7, 0x30).unwrap().0, pkcs7);
        let (_, cert) = rsa::pkcs8::Document::from_pem(CERT).unwrap();
        assert!(pkcs7.windows(cert.as_bytes().len()).any(|w| w == cert.as_bytes()));
        let public = RsaPublicKey::from_public_key_der(crate::der::Certificate::parse(cert.as_bytes()).unwrap().key).unwrap();
        let signature = &pkcs7[pkcs7.len() - 256..];
        public.verify(Pkcs1v15Sign::new::<Sha1>(), &Sha1::digest(signed), signature).unwrap();

        // Not an archive, an archive with a comment, and a certificate
        // that is not the key's.
        assert!(sign(b"PK", KEY, CERT).is_err());
        assert!(sign(&z, KEY, CERT).is_err());
        assert!(sign(&unsigned, KEY, KEY).is_err());
    }

    #[test]
    fn a_signed_package_is_verified_and_a_changed_one_is_not() {
        let unsigned = crate::zip::write(&[crate::zip::FileEntry { name: "a", data: &[7u8; 200_000], executable: false }]).unwrap();
        let z = sign(&unsigned, KEY, CERT).unwrap();
        let len = z.len() as u64;
        let mut seen = Vec::new();
        assert_eq!(verify(&mut std::io::Cursor::new(&z), len, &keys(), |p| seen.push(p)), Ok(0));
        assert!(seen.len() > 2 && seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0));
        assert!(has_footer(&mut std::io::Cursor::new(&z), len));

        // Not signed, signed and then changed, cut short, and followed by
        // other bytes.
        assert!(verify(&mut std::io::Cursor::new(&unsigned), unsigned.len() as u64, &keys(), |_| {}).unwrap_err().contains("not signed"));
        let mut changed = z.clone();
        changed[100] ^= 1;
        assert!(verify(&mut std::io::Cursor::new(&changed), len, &keys(), |_| {}).unwrap_err().contains("not by a key"));
        assert!(verify(&mut std::io::Cursor::new(&z), len - 1, &keys(), |_| {}).is_err());
        let mut longer = z.clone();
        longer.extend_from_slice(&[0; 256]);
        assert!(!has_footer(&mut std::io::Cursor::new(&longer), len + 256));
        assert!(has_footer(&mut std::io::Cursor::new(&longer), len));
        // No key, and a key that is not the signer's.
        assert!(verify(&mut std::io::Cursor::new(&z), len, &[], |_| {}).is_err());
        let other = RsaPublicKey::new(rsa::BigUint::from(0xc5u32) << 2040 | rsa::BigUint::from(1u32), rsa::BigUint::from(65537u32)).unwrap();
        assert!(verify(&mut std::io::Cursor::new(&z), len, &[other], |_| {}).is_err());
    }
}
