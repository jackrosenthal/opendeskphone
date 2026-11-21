//! Zip archives, which is what an OTA package is: reading files out of
//! one, and writing one of stored files.

use std::io::{Read, Seek, SeekFrom};

const EOCD_MARK: &[u8; 4] = b"PK\x05\x06";

/// One file of an archive.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// 0: stored; 8: deflated.
    method: u16,
    crc: u32,
    compressed: u64,
    pub size: u64,
    header: u64,
}

/// A zip archive in the first `len` bytes of a file.
pub struct Archive<R> {
    file: R,
    entries: Vec<Entry>,
}

fn u16_at(data: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([data[at], data[at + 1]]))
}

fn u32_at(data: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes(data[at..at + 4].try_into().unwrap()))
}

impl<R: Read + Seek> Archive<R> {
    pub fn open(mut file: R, len: u64) -> Result<Self, String> {
        let io = |e: std::io::Error| format!("reading the archive: {e}");
        // The end record, before a comment of at most 65535 bytes.
        let tail_len = len.min(22 + 65535);
        let mut tail = vec![0u8; tail_len as usize];
        file.seek(SeekFrom::Start(len - tail_len)).map_err(io)?;
        file.read_exact(&mut tail).map_err(io)?;
        let eocd = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|at| &tail[*at..*at + 4] == EOCD_MARK && at + 22 + u16_at(&tail, at + 20) as usize == tail.len())
            .ok_or("not a zip archive (no directory at its end)")?;
        let (count, size, start) = (u16_at(&tail, eocd + 10), u32_at(&tail, eocd + 12), u32_at(&tail, eocd + 16));
        if start + size > len || size > 64 << 20 {
            return Err("the archive's directory is not inside it".into());
        }
        let mut directory = vec![0u8; size as usize];
        file.seek(SeekFrom::Start(start)).map_err(io)?;
        file.read_exact(&mut directory).map_err(io)?;
        let mut entries = Vec::new();
        let mut at = 0;
        for _ in 0..count {
            if directory.len() < at + 46 || &directory[at..at + 4] != b"PK\x01\x02" {
                return Err("the archive's directory is damaged".into());
            }
            let d = &directory[at..];
            let (name_len, extra, comment) = (u16_at(d, 28) as usize, u16_at(d, 30) as usize, u16_at(d, 32) as usize);
            let name = d.get(46..46 + name_len).ok_or("the archive's directory is damaged")?;
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method: u16_at(d, 10) as u16,
                crc: u32_at(d, 16) as u32,
                compressed: u32_at(d, 20),
                size: u32_at(d, 24),
                header: u32_at(d, 42),
            });
            at += 46 + name_len + extra + comment;
        }
        Ok(Archive { file, entries })
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// One file's contents, which must be no longer than `limit`, checked
    /// against the archive's CRC of it.
    pub fn read(&mut self, name: &str, limit: usize) -> Result<Vec<u8>, String> {
        let io = |e: std::io::Error| format!("{name}: {e}");
        let e = self.entry(name).ok_or_else(|| format!("the package has no {name}"))?.clone();
        if e.size > limit as u64 || e.compressed > limit as u64 + 1024 {
            return Err(format!("{name} is {} bytes, more than the {limit} it may be", e.size));
        }
        let mut local = [0u8; 30];
        self.file.seek(SeekFrom::Start(e.header)).map_err(io)?;
        self.file.read_exact(&mut local).map_err(io)?;
        if &local[..4] != b"PK\x03\x04" {
            return Err(format!("{name}: the archive is damaged"));
        }
        let skip = u16_at(&local, 26) + u16_at(&local, 28);
        self.file.seek(SeekFrom::Current(skip as i64)).map_err(io)?;
        let mut data = vec![0u8; e.compressed as usize];
        self.file.read_exact(&mut data).map_err(io)?;
        let data = match e.method {
            0 => data,
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(&data, e.size as usize)
                .map_err(|_| format!("{name}: its compressed data is damaged"))?,
            other => return Err(format!("{name}: compressed in a way (method {other}) that is not read here")),
        };
        if data.len() as u64 != e.size || !eff::crc::crc(&data) != e.crc {
            return Err(format!("{name}: its contents do not fit the archive's checksum of them"));
        }
        Ok(data)
    }
}

/// One file of a zip archive.
pub struct FileEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    pub executable: bool,
}

/// A zip archive of stored (uncompressed) files, every one dated the first
/// day a zip can name, so that the same input makes the same archive.
pub fn write(entries: &[FileEntry]) -> Result<Vec<u8>, String> {
    // 1980-01-01, 00:00:00.
    const DATE: u16 = 1 << 5 | 1;
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for e in entries {
        let (len, name_len, offset) = (u32::try_from(e.data.len()), u16::try_from(e.name.len()), u32::try_from(out.len()));
        let (Ok(len), Ok(name_len), Ok(offset)) = (len, name_len, offset) else {
            return Err(format!("{}: too large for a zip archive", e.name));
        };
        let crc = !eff::crc::crc(e.data);
        // version needed (1.0), flags, method (stored), time, date, CRC,
        // both sizes, name length, extra length
        let mut common = Vec::new();
        for w in [10u16, 0, 0, 0, DATE] {
            common.extend_from_slice(&w.to_le_bytes());
        }
        for w in [crc, len, len] {
            common.extend_from_slice(&w.to_le_bytes());
        }
        common.extend_from_slice(&name_len.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&common);
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(e.data);

        // version made by: Unix, so that the mode is read
        directory.extend_from_slice(b"PK\x01\x02");
        directory.extend_from_slice(&(3u16 << 8 | 10).to_le_bytes());
        directory.extend_from_slice(&common);
        // comment length, disk, internal attributes
        directory.extend_from_slice(&[0; 6]);
        let mode: u32 = if e.executable { 0o100755 } else { 0o100644 };
        directory.extend_from_slice(&(mode << 16).to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(e.name.as_bytes());
    }
    let (count, start, size) = (u16::try_from(entries.len()), u32::try_from(out.len()), u32::try_from(directory.len()));
    let (Ok(count), Ok(start), Ok(size)) = (count, start, size) else {
        return Err("too large for a zip archive".into());
    };
    out.extend_from_slice(&directory);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(data: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_zip_has_its_files_stored() {
        let z = write(&[
            FileEntry { name: "a/b", data: b"123456789", executable: true },
            FileEntry { name: "c", data: b"", executable: false },
        ])
        .unwrap();
        assert_eq!(&z[..4], b"PK\x03\x04");
        // The CRC-32 of "123456789", and the data after the name.
        assert_eq!(word(&z, 14), 0xcbf4_3926);
        assert_eq!(&z[30..33], b"a/b");
        assert_eq!(&z[33..42], b"123456789");
        // The end record: two entries, and where the directory starts.
        let end = z.len() - 22;
        assert_eq!(&z[end..end + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[end + 10], z[end + 11]]), 2);
        let directory = word(&z, end + 16) as usize;
        assert_eq!(&z[directory..directory + 4], b"PK\x01\x02");
        assert_eq!(word(&z, directory + 38) >> 16, 0o100755);
        // The same input makes the same archive.
        assert_eq!(z, write(&[FileEntry { name: "a/b", data: b"123456789", executable: true }, FileEntry { name: "c", data: b"", executable: false }]).unwrap());
    }

    #[test]
    fn files_are_read_back_stored_or_deflated() {
        let mut z = write(&[
            FileEntry { name: "a/b", data: b"123456789", executable: true },
            FileEntry { name: "c", data: b"", executable: false },
        ])
        .unwrap();
        let len = z.len() as u64;
        let mut archive = Archive::open(std::io::Cursor::new(&z), len).unwrap();
        assert_eq!(archive.read("a/b", 9).unwrap(), b"123456789");
        assert_eq!(archive.read("c", 0).unwrap(), b"");
        assert!(archive.read("a/b", 8).unwrap_err().contains("more than"));
        assert!(archive.read("d", 9).unwrap_err().contains("no d"));
        // Bytes after the archive (the vendor's signature of an EFF image)
        // are not part of it.
        z.extend_from_slice(&[0xaa; 256]);
        assert!(Archive::open(std::io::Cursor::new(&z), len + 256).is_err());
        assert!(Archive::open(std::io::Cursor::new(&z), len).is_ok());
        // A damaged file does not fit its checksum.
        z[33] ^= 1;
        assert!(Archive::open(std::io::Cursor::new(&z), len).unwrap().read("a/b", 9).unwrap_err().contains("checksum"));

        // One deflated file, as another tool writes it.
        let data = b"abcabcabcabcabcabcabcabcabcabc".repeat(20);
        let packed = miniz_oxide::deflate::compress_to_vec(&data, 6);
        let mut z = Vec::new();
        let mut common = Vec::new();
        for w in [20u16, 0, 8, 0, 33] {
            common.extend_from_slice(&w.to_le_bytes());
        }
        for w in [!eff::crc::crc(&data), packed.len() as u32, data.len() as u32] {
            common.extend_from_slice(&w.to_le_bytes());
        }
        common.extend_from_slice(&[1, 0, 4, 0]);
        z.extend_from_slice(b"PK\x03\x04");
        z.extend_from_slice(&common);
        z.extend_from_slice(b"fEXTR");
        z.extend_from_slice(&packed);
        let start = z.len() as u32;
        z.extend_from_slice(b"PK\x01\x02\x14\x03");
        z.extend_from_slice(&common);
        z.extend_from_slice(&[0; 14]);
        z.extend_from_slice(b"fEXTR");
        let size = z.len() as u32 - start;
        z.extend_from_slice(b"PK\x05\x06\0\0\0\0\x01\0\x01\0");
        z.extend_from_slice(&size.to_le_bytes());
        z.extend_from_slice(&start.to_le_bytes());
        z.extend_from_slice(&[0, 0]);
        let len = z.len() as u64;
        assert_eq!(Archive::open(std::io::Cursor::new(&z), len).unwrap().read("f", 1000).unwrap(), data);
    }
}
