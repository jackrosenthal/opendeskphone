//! The signatures of an OTA package and of a D80 image.
//!
//! A package carries a whole-file signature, laid out the way Android's
//! recoveries expect it: a signed package is a zip archive whose comment
//! ends with the signature and six bytes, the distance from the end of
//! the file back to the start of the signature block, `ff ff`, and the
//! comment's length.  What is signed is the file up to the comment's
//! length field.  The signature is RSA, PKCS #1 v1.5, over the SHA-1 of
//! that, and it is the last bytes of the block: a recovery takes as many
//! bytes before the last six as the key is long, and does not read the
//! rest of the block.  The signature identifies the key a package was
//! built with and detects corruption; Android's recoveries verify it,
//! and this repository's recovery does not.
//!
//! A D80 image carries a second signature: the last 256 bytes of the
//! decrypted payload, after the package.  The stock updater verifies it;
//! [`sign_eff`] produces it and [`split_image`] removes it.

use eff::{EffHeader, EffWriter};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::io::{Read, Seek, Write, SeekFrom};

/// The certificate of the key that the image build signs a package with.
/// The key is in the repository, so the signature does not show who made a
/// package; see `crates/opendeskphone-image/keys/README.md`.
pub const CERTIFICATE: &str = include_str!("../../opendeskphone-image/keys/d80-ota.x509.pem");

/// The six bytes at the end, and the directory's end record before the
/// comment.
const FOOTER: u64 = 6;
const EOCD: u64 = 22;
const EOCD_MARK: &[u8; 4] = b"PK\x05\x06";

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

/// The size of the signature that follows the package in every D80
/// image.
pub const IMAGE_SIGNATURE: u64 = 256;

/// Builds a D80 image: the package in the stock envelope, encrypted with
/// the stock passphrase, followed by a 256-byte signature of the image.
/// The stock updater decrypts the image as a whole and verifies the
/// signature the way `openssl dgst -sha256 -verify` does, over the
/// decrypted bytes minus the last 256, before handing the package to the
/// recovery.  Every D80 image ends this way, stock and Open Desk Phone
/// alike, so the build signs ours with the same key that signs the
/// package.
///
/// `key` is that RSA private key (PKCS #8, PEM).  A fixed `salt` makes
/// the output reproducible: the same input produces the same image.
pub fn sign_eff(header: &EffHeader, package: &[u8], key: &str, salt: [u8; 8]) -> Result<Vec<u8>, String> {
    if !eff::envelope::is_d80(&header.model) {
        return Err(format!("image signatures apply only to the D80, not to {}", header.model));
    }
    if package.len() < 4 || package[..4] != *b"PK\x03\x04" {
        return Err("the payload of a D80 EFF must be an Android OTA package (zip)".into());
    }
    let key = RsaPrivateKey::from_pkcs8_pem(key).map_err(|e| format!("the signing key: {e}"))?;
    // The signature covers the decrypted image minus the signature
    // itself.
    let mut signed = Vec::with_capacity(eff::envelope::INNER_LEN + package.len());
    signed.extend_from_slice(&header.inner());
    signed.extend_from_slice(package);
    let signature = key
        .sign(Pkcs1v15Sign::new::<Sha256>(), &Sha256::digest(&signed))
        .map_err(|e| format!("signing the image: {e}"))?;
    let mut out = Vec::with_capacity(eff::envelope::LABEL_LEN + 16 + signed.len() + 16 + signature.len() + 16);
    let mut w = EffWriter::with_raw_headers(&mut out, &header.label(), &header.inner(), salt, eff::envelope::passphrase(&header.model))
        .map_err(|e| e.to_string())?;
    w.write_all(package).map_err(|e| e.to_string())?;
    // The signature is part of the plaintext: the updater finds it at the
    // end of the decrypted data.
    w.write_all(&signature).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Splits a D80 image's decrypted payload into the package and the
/// signature that follows it.  Returns an error if the package does not
/// end the way a signed package does, since such an image is not in the
/// format.
pub fn split_image(payload: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let at = payload
        .len()
        .checked_sub(IMAGE_SIGNATURE as usize)
        .ok_or("the image is shorter than its signature")?;
    let (package, signature) = payload.split_at(at);
    if !has_footer(&mut std::io::Cursor::new(package), at as u64) {
        return Err("the image does not contain a signed package followed by its signature".into());
    }
    Ok((package, signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eff::EffReader;

    const KEY: &str = include_str!("../../opendeskphone-image/keys/d80-ota.key.pem");
    const CERT: &str = CERTIFICATE;

    /// The signature that Android's recoveries verify (their
    /// `verifier.cpp`), with the key in the tree.
    #[test]
    fn a_signed_package_is_signed_as_android_verifies_it() {
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

    /// The layout that Android's recoveries expect, and that
    /// `split_image` looks for: has_footer finds the package's end in a
    /// signed package, but not past it.
    #[test]
    fn a_signed_package_has_the_expected_layout() {
        let unsigned = crate::zip::write(&[crate::zip::FileEntry { name: "a", data: &[7u8; 200_000], executable: false }]).unwrap();
        let z = sign(&unsigned, KEY, CERT).unwrap();
        let len = z.len() as u64;
        assert!(has_footer(&mut std::io::Cursor::new(&z), len));

        // Cut short, and followed by other bytes.
        assert!(!has_footer(&mut std::io::Cursor::new(&z), len - 1));
        let mut longer = z.clone();
        longer.extend_from_slice(&[0; 256]);
        assert!(!has_footer(&mut std::io::Cursor::new(&longer), len + 256));
        assert!(has_footer(&mut std::io::Cursor::new(&longer), len));
    }

    /// The stock updater's check of a D80 image, with the key in the
    /// tree: the stock passphrase decrypts it, `openssl dgst -sha256
    /// -verify` takes the decrypted bytes less the last 256, and the
    /// package is the 401st byte on.
    #[test]
    fn an_eff_is_what_the_stock_updater_verifies() {
        let header = EffHeader::new("D80", "3_1_0").unwrap();
        let package = sign(
            &crate::zip::write(&[crate::zip::FileEntry { name: "a", data: b"123456789", executable: false }]).unwrap(),
            KEY,
            CERT,
        )
        .unwrap();
        let image = sign_eff(&header, &package, KEY, [7; 8]).unwrap();
        // A fixed salt makes the image reproducible.
        assert_eq!(image, sign_eff(&header, &package, KEY, [7; 8]).unwrap());
        // The image is encrypted, so `split_image` takes the decrypted
        // payload rather than the whole file.
        assert!(split_image(&image).is_err());

        // openssl enc -d -aes-256-cbc -pass file:/system/etc/otapassphrase:
        // the reader consumes the four header fields for its own header,
        // and yields what the updater would see after them.
        let mut payload = Vec::new();
        EffReader::new(&image[..]).expect("the stock passphrase opens it").read_to_end(&mut payload).unwrap();
        let mut decrypted = header.inner().to_vec();
        decrypted.extend_from_slice(&payload);
        assert_eq!(decrypted.len(), eff::envelope::INNER_LEN + package.len() + 256);
        let (body, signature) = decrypted.split_at(decrypted.len() - 256);
        assert_eq!(&body[eff::envelope::INNER_LEN..], package);
        assert_eq!(split_image(&payload).unwrap(), (&package[..], signature));

        // openssl dgst -sha256 -verify, against the image and against one
        // byte of it changed.
        let key = RsaPrivateKey::from_pkcs8_pem(KEY).unwrap();
        let public = RsaPublicKey::from(&key);
        public.verify(Pkcs1v15Sign::new::<Sha256>(), &Sha256::digest(body), signature).unwrap();
        let mut changed = body.to_vec();
        changed[eff::envelope::INNER_LEN] ^= 1;
        assert!(public.verify(Pkcs1v15Sign::new::<Sha256>(), &Sha256::digest(&changed), signature).is_err());

        // Only a D80 image is signed, and the payload must be a package.
        assert!(sign_eff(&EffHeader::new("D60", "3_0_0").unwrap(), &package, KEY, [7; 8]).is_err());
        assert!(sign_eff(&header, b"not a zip", KEY, [7; 8]).is_err());
        assert!(split_image(&package).is_err());
    }
}
