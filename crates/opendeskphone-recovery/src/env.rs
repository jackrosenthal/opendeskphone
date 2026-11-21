//! The stock U-Boot's environment, in which `bootcmd` says whether the
//! phone starts the system or the recovery.
//!
//! The environment is one block in the eMMC's user area, before the first
//! partition: at [`OFFSET`], [`SIZE`] bytes, a little-endian CRC-32 of the
//! rest and then `name=value` strings, each ended by a zero, and one more
//! zero after the last.  U-Boot has no second copy.  When the CRC does not
//! fit it uses the environment it was built with, whose `bootcmd` starts
//! the system; so a block cut short by a loss of power starts the system.
//!
//! The stock firmware changes the block with its `uboot_env` tool: to
//! [`BOOT_RECOVERY`] before it restarts to install a package, and the stock
//! recovery back to [`BOOT_SYSTEM`] when it is done.

use std::fs::OpenOptions;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Where the block is on the eMMC.
pub const OFFSET: u64 = 0xC0000;
/// The block's length, CRC and all.
pub const SIZE: usize = 0x2000;
/// `bootcmd` to start the boot image in the first partition.
pub const BOOT_SYSTEM: &str = "booti mmc3";
/// `bootcmd` to start the boot image in the second partition.
pub const BOOT_RECOVERY: &str = "booti mmc3 recovery";

/// The variables, in the block's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env(Vec<(Vec<u8>, Vec<u8>)>);

fn crc(data: &[u8]) -> u32 {
    !eff::crc::crc(data)
}

impl Env {
    /// The variables of a block, or why it is not an environment (U-Boot
    /// would then use its own).
    pub fn parse(block: &[u8]) -> Result<Env, String> {
        if block.len() != SIZE {
            return Err(format!("the environment block is {} bytes, not {SIZE}", block.len()));
        }
        let stored = u32::from_le_bytes(block[..4].try_into().unwrap());
        if stored != crc(&block[4..]) {
            return Err("the environment's checksum does not fit it".into());
        }
        let mut vars = Vec::new();
        for var in block[4..].split(|b| *b == 0) {
            if var.is_empty() {
                break;
            }
            let Some(eq) = var.iter().position(|b| *b == b'=') else {
                return Err("the environment has a string that is not a variable".into());
            };
            vars.push((var[..eq].to_vec(), var[eq + 1..].to_vec()));
        }
        Ok(Env(vars))
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.0.iter().find(|(n, _)| n == name.as_bytes()).map(|(_, v)| String::from_utf8_lossy(v).into_owned())
    }

    /// Sets a variable where it is, or adds it after the others.
    pub fn set(&mut self, name: &str, value: &str) {
        match self.0.iter_mut().find(|(n, _)| n == name.as_bytes()) {
            Some((_, v)) => *v = value.as_bytes().to_vec(),
            None => self.0.push((name.as_bytes().to_vec(), value.as_bytes().to_vec())),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.0.iter().map(|(n, v)| (String::from_utf8_lossy(n).into_owned(), String::from_utf8_lossy(v).into_owned()))
    }

    /// The block to write.
    pub fn block(&self) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; 4];
        for (name, value) in &self.0 {
            out.extend_from_slice(name);
            out.push(b'=');
            out.extend_from_slice(value);
            out.push(0);
        }
        // The zero that ends the list.
        if out.len() + 1 > SIZE {
            return Err("the environment does not fit its block".into());
        }
        out.resize(SIZE, 0);
        let sum = crc(&out[4..]);
        out[..4].copy_from_slice(&sum.to_le_bytes());
        Ok(out)
    }
}

/// The environment on `disk` (the eMMC, [`crate::DISK`]).
pub fn read(disk: &Path) -> Result<Env, String> {
    let file = std::fs::File::open(disk).map_err(|e| format!("{}: {e}", disk.display()))?;
    let mut block = vec![0u8; SIZE];
    file.read_exact_at(&mut block, OFFSET).map_err(|e| format!("{}: {e}", disk.display()))?;
    Env::parse(&block)
}

/// Sets `bootcmd` in the environment on `disk`, and reads the block back.
/// Returns whether the block was written: it is left alone when it says so
/// already.  An environment whose checksum does not fit is an error, and is
/// not written: U-Boot does not use it.
pub fn set_bootcmd(disk: &Path, command: &str) -> Result<bool, String> {
    let at = |e: std::io::Error| format!("{}: {e}", disk.display());
    let file = OpenOptions::new().read(true).write(true).open(disk).map_err(at)?;
    let mut block = vec![0u8; SIZE];
    file.read_exact_at(&mut block, OFFSET).map_err(at)?;
    let mut env = Env::parse(&block)?;
    if env.get("bootcmd").as_deref() == Some(command) {
        return Ok(false);
    }
    env.set("bootcmd", command);
    let new = env.block()?;
    file.write_all_at(&new, OFFSET).map_err(at)?;
    file.sync_all().map_err(at)?;
    file.read_exact_at(&mut block, OFFSET).map_err(at)?;
    if block != new {
        return Err(format!("{}: the environment read back is not the one written", disk.display()));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block as the stock U-Boot wrote it on the phone the port was done
    /// on: the variables that matter, and the checksum's placement.
    fn stock() -> Vec<u8> {
        let mut env = Env(Vec::new());
        env.set("bootdelay", "3");
        env.set("bootargs", "console=ttymxc0,115200 init=/init");
        env.set("bootcmd", BOOT_SYSTEM);
        env.block().unwrap()
    }

    #[test]
    fn the_checksum_is_the_crc32_of_what_follows_it() {
        let block = stock();
        // CRC-32 as zlib computes it, of a known string.
        assert_eq!(crc(b"123456789"), 0xcbf4_3926);
        assert_eq!(u32::from_le_bytes(block[..4].try_into().unwrap()), crc(&block[4..]));
        assert_eq!(&block[4..16], b"bootdelay=3\0");
    }

    #[test]
    fn bootcmd_is_changed_in_place() {
        let mut env = Env::parse(&stock()).unwrap();
        env.set("bootcmd", BOOT_RECOVERY);
        let again = Env::parse(&env.block().unwrap()).unwrap();
        assert_eq!(again.get("bootcmd").as_deref(), Some(BOOT_RECOVERY));
        assert_eq!(again.names().map(|(n, _)| n).collect::<Vec<_>>(), ["bootdelay", "bootargs", "bootcmd"]);
    }

    #[test]
    fn a_damaged_block_is_not_an_environment() {
        let mut block = stock();
        block[100] ^= 1;
        assert!(Env::parse(&block).is_err());
        assert!(Env::parse(&[0u8; SIZE]).is_err());
    }

    #[test]
    fn the_block_on_a_disk_is_rewritten_only_when_it_differs() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("disk");
        let mut image = vec![0xa5u8; OFFSET as usize + SIZE + 512];
        image[OFFSET as usize..OFFSET as usize + SIZE].copy_from_slice(&stock());
        std::fs::write(&disk, &image).unwrap();
        assert_eq!(set_bootcmd(&disk, BOOT_SYSTEM), Ok(false));
        assert_eq!(set_bootcmd(&disk, BOOT_RECOVERY), Ok(true));
        assert_eq!(read(&disk).unwrap().get("bootcmd").as_deref(), Some(BOOT_RECOVERY));
        // Nothing around the block is touched.
        let after = std::fs::read(&disk).unwrap();
        assert!(after[..OFFSET as usize].iter().chain(&after[OFFSET as usize + SIZE..]).all(|b| *b == 0xa5));
        // A block U-Boot would not use is left as it is.
        image[OFFSET as usize + 9] ^= 1;
        std::fs::write(&disk, &image).unwrap();
        assert!(set_bootcmd(&disk, BOOT_RECOVERY).is_err());
        assert_eq!(std::fs::read(&disk).unwrap(), image);
    }
}
