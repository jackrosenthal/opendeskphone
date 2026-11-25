//! On-device firmware update for Open Desk Phone (plan section 12): install an
//! EFF into the spare bank, maintain boot2's `ctrl` record, and support
//! trial boots with rollback.
//!
//! - [`install::Updater`]: write an EFF (ours or stock) into the bank that
//!   is not running, read it back and compare, and select it.  A file is
//!   checked in full before anything is written; a stream goes straight
//!   into the spare bank.  On a D80, which has no banks, the image is left
//!   for its recovery instead (`d80`, with the feature of that name).
//! - [`lock`]: one update at a time.
//! - [`ctrl`]: the `ctrl` partition's replication layout per family and a
//!   power-cut-tolerant writer (shared with the early-boot stage through
//!   `opendeskphone-ctrl`).
//! - [`trial`]: the pending-trial record in nvdata and the rollback the
//!   init role calls.  The early-boot stage in the kernel partition
//!   (`opendeskphone-early`) counts the boots.
//! - [`inspect`]: read-only status, and the read-back check of a bank.
//! - [`boot2`]: a model of the stock second-stage loader's decisions, used
//!   to refuse images and states it would not boot.
//! - [`ramboot`]: lay out an EFF's kernel and root file system for
//!   `kexec`, to try an image without installing it.
//!
//! Never touched: `boot1`, `boot2`, the partition table, `nvdata` contents
//! other than `/nvdata/opendeskphone/trial`.  Bootstrap (type 1) OTAs are
//! rejected by the OTA parser.
//!
//! Typical use by the main role (the updater takes the lock, refuses while
//! an installed image is still on trial, and records the trial itself):
//!
//! ```ignore
//! Updater::new(hal.mtd.as_mut(), model, &cmdline, &own_dir)
//!     .store(store.clone())
//!     .install_eff(upload)?;
//! // then reboot
//! ```
//!
//! and by the init role's rollback hook (`fn(&TrialRecord) -> io::Result<()>`):
//!
//! ```ignore
//! trial::rollback(mtd, model, &cmdline, previous_bank).map_err(Into::into)
//! ```

pub mod bank;
pub mod boot2;
pub mod ctrl;
#[cfg(feature = "d80")]
pub mod d80;
pub mod error;
mod flash;
pub mod inspect;
pub mod install;
pub mod lock;
pub mod ramboot;
pub mod trial;

#[cfg(test)]
mod memnand;
#[cfg(test)]
mod tests;

pub use bank::Bank;
pub use ctrl::{Replication, replication_for};
pub use error::{Result, UpdateError};
pub use inspect::{BankStatus, Status, inspect, verify_bank};
pub use install::{D80_NOT_HERE, Installed, Options, Staged, Target, Updater, abandon, check_eff, is_stock_version, select_bank};
