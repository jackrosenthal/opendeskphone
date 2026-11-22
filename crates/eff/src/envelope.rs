//! The `.eff` envelope.
//!
//! ```text
//! 0x00  100-byte plaintext label "<model> <version>", NUL padded
//! 0x64  "Salted__", 8-byte salt
//! 0x74  AES-256-CBC ciphertext, PKCS#7 padded, of:
//!         100 bytes "Digium Phone Firmware"
//!         100 bytes "1"
//!         100 bytes model
//!         100 bytes version
//!         the payload
//! ```
//!
//! Key and IV come from OpenSSL's legacy `EVP_BytesToKey` with MD5 and one
//! iteration over the passphrase in the stock `/etc/otapassphrase`, which
//! the stock updater hands to `openssl enc -pass file:`; OpenSSL takes the
//! file's first line, without its newline.
//!
//! The D80 differs from the other models in two ways: its passphrase is
//! [`D80_OTA_PASSPHRASE`], and its payload is an Android OTA package (a
//! zip file) instead of an OTA container, followed by a signature of the
//! whole image (`opendeskphone-recovery::sig` builds such images and
//! splits them apart).  See [`is_d80`].

use crate::{Error, Result, bad};
use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use md5::{Digest, Md5};
use std::io::{Read, Write};

/// The passphrase in the stock `/etc/otapassphrase` of the D40 through
/// D70: the file's first line, without its newline (all but its last byte).
pub const OTA_PASSPHRASE: &[u8; 255] =b"*\xac7\xf5\x9e1\xb0\xde\xef\xfb1\xf4N`V#\x82\xa1%\xb6$)\xd9\x12\xc3\xb3\xc4\x89\xbd1O\xb4\xac|\xcf\xb0\"k\x92\xf5\x14K'T\x8e \x8c\xe2\x89\xe9\x03#>\x84\x0e0\x11,~\xd8\xdc#3~}\xb9m&\xad\x18|~J#G)j\x17\x8e\xe0\x07\xa3\xb7k\xa7\xa6BF0@?MEv\xad\xa24\xc4 \x06-t\xf2\xb8q\xf8\xb9\x8d \x16\x1a\xa5}\x18\xdc\x03)\xc3\xb0\x03\x03]q\x0f\xbaI<\xf3\xf4Q\xb2\xfb\xe3\xf1\x8e(\xc5\xa2\xfdX\xf8\xb1\xb7\x17\x9cL \xca0\xf0:\x07\xd80\xb9\x97\x06\x17\x89\x01\x03v\xc5\x1c\xc0\xb2\xed-\xff\xd9\x15_+\xe1}\x9d\x95&~\xe4\xdc\xcd\x061Uf\x90 VR\xa0 \xd6\xcf\xec\x1eP\xcf\xdaU\xe0\xa1\x14\xe7t\\N\x06\x1b\xab\xf5\xe7\x1f\x85\x9f\xd05\xa3\xd1QY\xe1\x10\xe9\xcc\xdf_d\x87\xcf\xa4\x9c5$\xbdq_\xdb\xee\x1a\x07^\xeb\xc7G\x90\xdc{\xa1\x88\xef=\xb6Wq";

/// The passphrase in the stock `/etc/otapassphrase` of the D80: the file's
/// first line, without its newline (47 of its 256 bytes).
pub const D80_OTA_PASSPHRASE: &[u8; 47] = b"q0\xcev\xf5\"\x16\xd15N7\xee\xbb\x0ez\xec\x8d\xf3\x82\xb5\xc0z\x9b\xb2>_8u\x9d\x12\xa6\xc3%R\xde\xddz/\x84j0\x8d)0\xf4\xde\xc8";

/// Whether `model` is the D80, whose EFFs use [`D80_OTA_PASSPHRASE`] and
/// carry an Android OTA package (a zip file) instead of an OTA container.
pub fn is_d80(model: &str) -> bool {
    model.eq_ignore_ascii_case("D80")
}

/// The passphrase of the EFFs for `model`.
pub fn passphrase(model: &str) -> &'static [u8] {
    if is_d80(model) {
        D80_OTA_PASSPHRASE
    } else {
        OTA_PASSPHRASE
    }
}

/// Check that `payload` starts the way the payload of an EFF for `model`
/// must: with the zip magic for the D80, with the OTA magic otherwise.
fn check_payload(model: &str, payload: &[u8]) -> Result<()> {
    let (magic, what) = if is_d80(model) {
        (*b"PK\x03\x04", "an Android OTA package (zip)")
    } else {
        (crate::OTA_MAGIC.to_be_bytes(), "an OTA container")
    };
    if payload.len() < 4 || payload[..4] != magic {
        return bad(format!("the payload of a {model} EFF must be {what}"));
    }
    Ok(())
}

/// The model the plaintext label names (its text before the first space).
fn label_model(label: &[u8; LABEL_LEN]) -> &str {
    let end = label
        .iter()
        .position(|&b| b == b' ' || b == 0)
        .unwrap_or(LABEL_LEN);
    std::str::from_utf8(&label[..end]).unwrap_or("")
}

pub const LABEL_LEN: usize = 100;
pub const INNER_LEN: usize = 400;
const FIELD: usize = 100;
const MAGIC_TEXT: &str = "Digium Phone Firmware";
const ENVELOPE_VERSION: &str = "1";
const SALTED: &[u8; 8] = b"Salted__";
const BS: usize = 16;

fn field(value: &str) -> Result<[u8; FIELD]> {
    let b = value.as_bytes();
    if b.is_empty() || b.len() >= FIELD || !value.is_ascii() || b.contains(&0) {
        return bad("header fields must contain 1 to 99 non-NUL ASCII bytes");
    }
    let mut out = [0u8; FIELD];
    out[..b.len()].copy_from_slice(b);
    Ok(out)
}

fn field_text(b: &[u8]) -> Result<String> {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    match std::str::from_utf8(&b[..end]) {
        Ok(s) if s.is_ascii() => Ok(s.to_owned()),
        _ => bad("EFF header field is not ASCII"),
    }
}

/// Model and version carried by an EFF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffHeader {
    /// E.g., `D60`.  The stock updater compares this with `/etc/phone-model`.
    pub model: String,
    /// E.g., `2_9_28` or `3_0_0`.
    pub version: String,
}

impl EffHeader {
    pub fn new(model: &str, version: &str) -> Result<Self> {
        field(model)?;
        field(version)?;
        field(&format!("{model} {version}"))?;
        Ok(EffHeader {
            model: model.into(),
            version: version.into(),
        })
    }

    /// The plaintext outer label, `<model> <version>`.
    pub fn label(&self) -> [u8; LABEL_LEN] {
        field(&format!("{} {}", self.model, self.version)).expect("checked in new")
    }

    /// The four encrypted 100-byte fields.
    pub fn inner(&self) -> [u8; INNER_LEN] {
        let mut out = [0u8; INNER_LEN];
        out[0..100].copy_from_slice(&field(MAGIC_TEXT).unwrap());
        out[100..200].copy_from_slice(&field(ENVELOPE_VERSION).unwrap());
        out[200..300].copy_from_slice(&field(&self.model).expect("checked in new"));
        out[300..400].copy_from_slice(&field(&self.version).expect("checked in new"));
        out
    }

    fn parse_inner(b: &[u8; INNER_LEN]) -> Result<Self> {
        if field_text(&b[0..100])? != MAGIC_TEXT || field_text(&b[100..200])? != ENVELOPE_VERSION {
            return bad("unsupported decrypted firmware header or version (wrong passphrase?)");
        }
        Ok(EffHeader {
            model: field_text(&b[200..300])?,
            version: field_text(&b[300..400])?,
        })
    }
}

/// `EVP_BytesToKey(aes-256-cbc, md5, salt, passphrase, count = 1)`.
fn derive(passphrase: &[u8], salt: &[u8; 8]) -> ([u8; 32], [u8; 16]) {
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
    (
        derived[..32].try_into().unwrap(),
        derived[32..].try_into().unwrap(),
    )
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

const CHUNK: usize = 16 * 1024;

/// Streaming EFF decryptor.  Construction reads and checks the label, the
/// salt, and the four inner fields; reading then yields the OTA.  The final
/// PKCS#7 padding is checked when the stream ends.
pub struct EffReader<R: Read> {
    inner: R,
    cipher: Aes256,
    prev: [u8; BS],
    header: EffHeader,
    label: [u8; LABEL_LEN],
    salt: [u8; 8],
    /// Decrypted bytes not yet handed out: `buf[pos..len]`.
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    /// One ciphertext block read ahead, to know which block is the last.
    ahead: Option<[u8; BS]>,
    done: bool,
}

impl<R: Read> EffReader<R> {
    /// Open with the stock passphrase of the model the plaintext label
    /// names (see [`passphrase`]).
    pub fn new(inner: R) -> Result<Self> {
        Self::open(inner, None)
    }

    pub fn with_passphrase(inner: R, passphrase: &[u8]) -> Result<Self> {
        Self::open(inner, Some(passphrase))
    }

    fn open(mut inner: R, passphrase: Option<&[u8]>) -> Result<Self> {
        let mut head = [0u8; LABEL_LEN + 16];
        if read_full(&mut inner, &mut head)? != head.len() || &head[100..108] != SALTED {
            return bad("invalid EFF framing (100-byte label and salted AES ciphertext expected)");
        }
        let label: [u8; LABEL_LEN] = head[..100].try_into().unwrap();
        let salt: [u8; 8] = head[108..116].try_into().unwrap();
        let passphrase = passphrase.unwrap_or_else(|| self::passphrase(label_model(&label)));
        let (key, iv) = derive(passphrase, &salt);
        let mut r = EffReader {
            inner,
            cipher: Aes256::new(GenericArray::from_slice(&key)),
            prev: iv,
            header: EffHeader {
                model: String::new(),
                version: String::new(),
            },
            label,
            salt,
            buf: vec![0u8; CHUNK],
            pos: 0,
            len: 0,
            ahead: None,
            done: false,
        };
        let mut inner_hdr = [0u8; INNER_LEN];
        let mut got = 0;
        while got < INNER_LEN {
            let n = r.read_plain(&mut inner_hdr[got..])?;
            if n == 0 {
                return bad("truncated decrypted firmware");
            }
            got += n;
        }
        r.header = EffHeader::parse_inner(&inner_hdr)?;
        Ok(r)
    }

    /// Model and version from the encrypted header (the ones stock checks).
    pub fn header(&self) -> &EffHeader {
        &self.header
    }

    /// The plaintext outer label (informational; stock does not trust it).
    pub fn label(&self) -> &[u8; LABEL_LEN] {
        &self.label
    }

    pub fn salt(&self) -> &[u8; 8] {
        &self.salt
    }

    fn fill(&mut self) -> Result<()> {
        self.pos = 0;
        self.len = 0;
        if self.done {
            return Ok(());
        }
        // Assemble ciphertext: the block read ahead, then fresh data.
        let mut have = 0;
        if let Some(a) = self.ahead.take() {
            self.buf[..BS].copy_from_slice(&a);
            have = BS;
        }
        have += read_full(&mut self.inner, &mut self.buf[have..])?;
        if have % BS != 0 {
            return bad("EFF ciphertext is not a whole number of AES blocks");
        }
        if have == 0 {
            return bad("EFF ciphertext is empty");
        }
        // Hold one block back unless the stream is known to have ended.
        let at_end = have < self.buf.len();
        let (mut usable, final_block) = if at_end {
            (have, true)
        } else {
            let mut a = [0u8; BS];
            let n = read_full(&mut self.inner, &mut a)?;
            if n == 0 {
                (have, true)
            } else if n != BS {
                return bad("EFF ciphertext is not a whole number of AES blocks");
            } else {
                self.ahead = Some(a);
                (have, false)
            }
        };
        for block in self.buf[..usable].as_chunks_mut::<BS>().0 {
            let ct: [u8; BS] = *block;
            self.cipher
                .decrypt_block(GenericArray::from_mut_slice(&mut block[..]));
            for (b, p) in block.iter_mut().zip(self.prev.iter()) {
                *b ^= p;
            }
            self.prev = ct;
        }
        if final_block {
            self.done = true;
            let pad = self.buf[usable - 1] as usize;
            if pad == 0
                || pad > BS
                || self.buf[usable - pad..usable]
                    .iter()
                    .any(|&b| b as usize != pad)
            {
                return bad("bad EFF padding (corrupt file or wrong passphrase)");
            }
            usable -= pad;
        }
        self.len = usable;
        Ok(())
    }

    fn read_plain(&mut self, out: &mut [u8]) -> Result<usize> {
        while self.pos == self.len {
            if self.done {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = out.len().min(self.len - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl<R: Read> Read for EffReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.read_plain(buf).map_err(Into::into)
    }
}

/// Streaming EFF encryptor: write the OTA, then call [`EffWriter::finish`].
pub struct EffWriter<W: Write> {
    inner: W,
    cipher: Aes256,
    prev: [u8; BS],
    pending: Vec<u8>,
}

impl<W: Write> EffWriter<W> {
    /// Start an EFF with the stock passphrase of the header's model (see
    /// [`passphrase`]).  `salt` should be random (any value decrypts; a
    /// fixed one makes the output reproducible).
    pub fn new(inner: W, header: &EffHeader, salt: [u8; 8]) -> Result<Self> {
        Self::with_raw_headers(
            inner,
            &header.label(),
            &header.inner(),
            salt,
            passphrase(&header.model),
        )
    }

    /// Start an EFF from header bytes given verbatim (used for byte-exact
    /// reconstruction of vendor files).
    pub fn with_raw_headers(
        mut inner: W,
        label: &[u8; LABEL_LEN],
        inner_header: &[u8; INNER_LEN],
        salt: [u8; 8],
        passphrase: &[u8],
    ) -> Result<Self> {
        EffHeader::parse_inner(inner_header)?;
        let (key, iv) = derive(passphrase, &salt);
        inner.write_all(label)?;
        inner.write_all(SALTED)?;
        inner.write_all(&salt)?;
        let mut w = EffWriter {
            inner,
            cipher: Aes256::new(GenericArray::from_slice(&key)),
            prev: iv,
            pending: Vec::with_capacity(CHUNK + BS),
        };
        w.write_all(inner_header)?;
        Ok(w)
    }

    fn encrypt_pending(&mut self, upto: usize) -> std::io::Result<()> {
        for block in self.pending[..upto].as_chunks_mut::<BS>().0 {
            for (b, p) in block.iter_mut().zip(self.prev.iter()) {
                *b ^= p;
            }
            self.cipher
                .encrypt_block(GenericArray::from_mut_slice(&mut block[..]));
            self.prev = *block;
        }
        self.inner.write_all(&self.pending[..upto])?;
        self.pending.drain(..upto);
        Ok(())
    }

    /// Write the padding and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        let pad = BS - self.pending.len() % BS;
        self.pending.extend(std::iter::repeat_n(pad as u8, pad));
        let n = self.pending.len();
        self.encrypt_pending(n)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for EffWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        let whole = self.pending.len() / BS * BS;
        if whole >= CHUNK {
            self.encrypt_pending(whole)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Decrypt a whole EFF in memory: header and payload bytes.
pub fn unpack(eff: &[u8]) -> Result<(EffHeader, Vec<u8>)> {
    let mut r = EffReader::new(eff)?;
    let mut payload = Vec::with_capacity(eff.len());
    r.read_to_end(&mut payload).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => Error::Format(e.to_string()),
        _ => Error::Io(e),
    })?;
    check_payload(&r.header().model, &payload)?;
    Ok((r.header().clone(), payload))
}

/// Encrypt a payload (an OTA container, or for the D80 an Android OTA
/// package) into an EFF in memory.
pub fn pack(header: &EffHeader, payload: &[u8], salt: [u8; 8]) -> Result<Vec<u8>> {
    check_payload(&header.model, payload)?;
    let mut w = EffWriter::new(Vec::with_capacity(payload.len() + 600), header, salt)?;
    w.write_all(payload)?;
    w.finish()
}
