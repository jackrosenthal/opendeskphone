//! As much of DER as a package's signature needs: reading one element, and
//! finding the parts of an X.509 certificate.

/// The DER element at the start of `data`, which must have the tag `tag`:
/// the whole element, its content, and what follows it.
pub fn element(data: &[u8], tag: u8) -> Option<(&[u8], &[u8], &[u8])> {
    let (&found, &first) = (data.first()?, data.get(1)?);
    let (header, len) = if first < 0x80 {
        (2, usize::from(first))
    } else {
        let n = usize::from(first & 0x7f);
        let bytes = data.get(2..2 + n).filter(|_| n <= 4)?;
        (2 + n, bytes.iter().fold(0, |len, b| len << 8 | usize::from(*b)))
    };
    let end = header.checked_add(len).filter(|end| *end <= data.len())?;
    (found == tag).then(|| (&data[..end], &data[header..end], &data[end..]))
}

/// What a signature names of an X.509 certificate.
pub struct Certificate<'a> {
    pub der: &'a [u8],
    /// The serial number and the issuer's name, each as a DER element.
    pub serial: &'a [u8],
    pub issuer: &'a [u8],
    /// The SubjectPublicKeyInfo.
    pub key: &'a [u8],
}

impl<'a> Certificate<'a> {
    pub fn parse(der: &'a [u8]) -> Option<Self> {
        const SEQUENCE: u8 = 0x30;
        let (_, certificate, _) = element(der, SEQUENCE)?;
        let (_, mut rest, _) = element(certificate, SEQUENCE)?;
        // The version, which a version 1 certificate leaves out.
        if let Some((_, _, after)) = element(rest, 0xa0) {
            rest = after;
        }
        let (serial, _, rest) = element(rest, 0x02)?;
        let (_, _, rest) = element(rest, SEQUENCE)?; // the signature's algorithm
        let (issuer, _, rest) = element(rest, SEQUENCE)?;
        let (_, _, rest) = element(rest, SEQUENCE)?; // the validity
        let (_, _, rest) = element(rest, SEQUENCE)?; // the subject
        let (key, _, _) = element(rest, SEQUENCE)?;
        Some(Certificate { der, serial, issuer, key })
    }
}
