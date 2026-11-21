//! The `ctrl` partition: where the bank-selection record lives and how it
//! is replicated.
//!
//! The 304-byte record itself is [`eff::ctrl::CtrlRecord`].  In flash it is
//! padded to one NAND page with `0xff`.
//!
//! The walk over the partition and the power-cut-tolerant writer are in
//! `opendeskphone-ctrl`, which the early-boot stage in the kernel partition
//! uses too: the two write the record in exactly the same way.  This module
//! puts the updater's types around them.

use opendeskphone_ctrl::Flash;
pub use opendeskphone_ctrl::Replication;
use opendeskphone_hal::{HalError, MtdPartition};
use opendeskphone_state::Family;
use eff::ctrl::{CtrlRecord, Side};

use crate::error::{Result, UpdateError};

/// Name of the partition.
pub const CTRL_PART: &str = "ctrl";

/// The layout of copies within `ctrl` for a family: one copy at the start
/// of every good block (old family: D40, D45, D50, D70; four copies seen
/// on a D40 and a D50), or a copy in every page of every good block (new
/// family: D60, D62, D65; 256 identical pages seen in a D60 dump and on a
/// live D60 after a stock update).
pub const fn replication_for(family: Family) -> Replication {
    match family {
        Family::Old => Replication::BlockStart,
        Family::New => Replication::EveryPage,
        // The D80 has no `ctrl` partition (its images are Android OTA
        // packages), so this is never asked for one.
        Family::Touch => Replication::EveryPage,
    }
}

/// An MTD partition as `opendeskphone-ctrl` sees flash.
struct Part<'a>(&'a mut dyn MtdPartition);

impl Flash for Part<'_> {
    type Error = HalError;
    fn geometry(&self) -> opendeskphone_ctrl::Geometry {
        let info = self.0.info();
        opendeskphone_ctrl::Geometry { size: info.size, erase_size: info.erase_size, write_size: info.write_size }
    }
    fn is_bad(&mut self, offset: u64) -> std::result::Result<bool, HalError> {
        self.0.is_bad(offset)
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> std::result::Result<(), HalError> {
        self.0.read(offset, buf)
    }
    fn erase(&mut self, offset: u64) -> std::result::Result<(), HalError> {
        self.0.erase(offset)
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> std::result::Result<(), HalError> {
        self.0.write(offset, data)
    }
}

/// What a walk over `ctrl` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtrlScan {
    /// The record boot2 would use: the first CRC-valid first-page copy.
    pub record: Option<CtrlRecord>,
    /// Index of the block that copy came from.
    pub record_block: Option<u32>,
    pub good_blocks: u32,
    pub bad_blocks: u32,
    /// Good blocks whose first page holds a CRC-valid record.
    pub valid_copies: u32,
    /// All CRC-valid first-page copies are identical.
    pub consistent: bool,
    /// What boot2 is left with when no copy is valid: it keeps going with
    /// the last page it read, unchecked.
    pub fallback: Option<CtrlRecord>,
}

impl CtrlScan {
    /// The record boot2 acts on, checked or not.
    pub fn effective(&self) -> Option<&CtrlRecord> {
        self.record.as_ref().or(self.fallback.as_ref())
    }
}

/// A record's fields without the CRC check (what boot2 has in its buffer
/// when it prints "Control Block is invalid" and carries on).
fn unchecked(b: &[u8]) -> CtrlRecord {
    CtrlRecord {
        active: u32::from_be_bytes(b[0..4].try_into().unwrap()),
        sides: [Side { raw: b[4..152].try_into().unwrap() }, Side { raw: b[152..300].try_into().unwrap() }],
    }
}

/// Walks `ctrl` the way boot2 does.
pub fn scan(part: &mut dyn MtdPartition) -> Result<CtrlScan> {
    let raw = opendeskphone_ctrl::scan(&mut Part(part))?;
    Ok(CtrlScan {
        // A copy the walk found valid parses.
        record: raw.record.as_ref().and_then(|b| CtrlRecord::parse(b).ok()),
        record_block: raw.record_block,
        good_blocks: raw.good_blocks,
        bad_blocks: raw.bad_blocks,
        valid_copies: raw.valid_copies,
        consistent: raw.consistent,
        fallback: raw.fallback.as_ref().map(|b| unchecked(b)),
    })
}

/// The record boot2 would use; an error when no copy is valid.
pub fn read(part: &mut dyn MtdPartition) -> Result<CtrlRecord> {
    scan(part)?.record.ok_or_else(|| UpdateError::Ctrl("no valid copy in the ctrl partition".into()))
}

/// Writes `record` to every good block of `ctrl`, one block at a time, and
/// confirms that boot2's walk now yields exactly this record
/// ([`opendeskphone_ctrl::write`]).
pub fn write(part: &mut dyn MtdPartition, record: &CtrlRecord, repl: Replication) -> Result<()> {
    use opendeskphone_ctrl::WriteError;
    let info = part.info().clone();
    let mut scratch = vec![0u8; 2 * info.erase_size as usize];
    match opendeskphone_ctrl::write(&mut Part(part), &record.to_bytes(), repl, &mut scratch) {
        Ok(0) => Ok(()),
        Ok(failed) => {
            log::warn!("ctrl: {failed} block(s) failed but hold no stale record");
            Ok(())
        }
        Err(WriteError::Flash(e)) => Err(e.into()),
        Err(WriteError::Geometry) => Err(UpdateError::Layout(format!(
            "ctrl geometry (page {}, block {}) cannot hold the record",
            info.write_size, info.erase_size
        ))),
        Err(WriteError::BadActive) => {
            Err(UpdateError::Refused(format!("active side {} is not 0 or 1", record.active)))
        }
        Err(WriteError::NoBlockWritten) => Err(UpdateError::Ctrl("no ctrl block could be written".into())),
        Err(WriteError::NotReadBack { failed, good }) => Err(UpdateError::Ctrl(format!(
            "after writing, boot2 would not read the new record ({failed} of {good} good blocks failed)"
        ))),
    }
}

/// Which side boot2 boots given what a scan found (the same in both
/// families).  With no valid copy boot2 applies the same rule to the
/// unchecked buffer.
pub fn boot_side(scan: &CtrlScan) -> u32 {
    scan.effective().map_or(0, CtrlRecord::boot_side)
}
