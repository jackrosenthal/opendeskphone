//! What the running system leaves for the recovery, and the recovery for
//! the running system: files in the directory `recovery` of the stock
//! `/cache` partition, as Android's recovery has them.

/// Where the recovery mounts the partition (`dphone` has it at `/nvdata`).
pub const CACHE: &str = "/cache";
/// The directory, in the partition.
pub const DIR: &str = "recovery";
/// The recovery's arguments, one to a line.
pub const COMMAND: &str = "command";
/// The package `dphone` leaves.
pub const PACKAGE: &str = "update.zip";
/// The package the recovery was last told to install, and on the second
/// line `1` if it installed it, `0` if not.
pub const LAST_INSTALL: &str = "last_install";
/// The recovery's log of its last run.
pub const LAST_LOG: &str = "last_log";
/// The [`crate::checksum`] of the package the recovery last installed,
/// which is what the system runs.
pub const INSTALLED: &str = "installed";

/// The command that has the recovery install the package `dphone` left.
pub fn install_command() -> String {
    format!("--update_package={CACHE}/{DIR}/{PACKAGE}\n")
}

/// The recovery's arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// The package to install, as a path in the recovery.
    pub update_package: Option<String>,
    /// Arguments this recovery does not act on.
    pub ignored: Vec<String>,
}

/// The arguments in a command file.  The stock firmware's first line is
/// `boot-recovery`, which is not an option, and which Android's recovery
/// passes over; so is everything else that is not known here.
pub fn parse(text: &str) -> Args {
    let mut args = Args::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match line.strip_prefix("--update_package=") {
            // Android's own older name for the partition.
            Some(path) => {
                args.update_package =
                    Some(path.strip_prefix("CACHE:").map(|rest| format!("{CACHE}/{rest}")).unwrap_or_else(|| path.to_string()));
            }
            None => args.ignored.push(line.to_string()),
        }
    }
    args
}

/// What a `last_install` file says: the package, and whether it was
/// installed.
pub fn parse_last_install(text: &str) -> Option<(String, bool)> {
    let mut lines = text.lines();
    let package = lines.next()?.to_string();
    Some((package, lines.next()?.trim() == "1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_file_is_one_argument_to_a_line() {
        assert_eq!(parse(&install_command()).update_package.as_deref(), Some("/cache/recovery/update.zip"));
        // As the stock firmware writes it.
        let stock = parse("boot-recovery\n--update_package=/data/digium/firmware/update.zip\n--wipe_data\n");
        assert_eq!(stock.update_package.as_deref(), Some("/data/digium/firmware/update.zip"));
        assert_eq!(stock.ignored, ["boot-recovery", "--wipe_data"]);
        assert_eq!(parse("--update_package=CACHE:a.zip").update_package.as_deref(), Some("/cache/a.zip"));
        assert_eq!(parse(""), Args::default());
    }

    #[test]
    fn last_install_names_the_package_and_the_result() {
        assert_eq!(parse_last_install("/cache/recovery/update.zip\n1\n"), Some(("/cache/recovery/update.zip".into(), true)));
        assert_eq!(parse_last_install("/cache/x.zip\n0\n"), Some(("/cache/x.zip".into(), false)));
        assert_eq!(parse_last_install("/cache/x.zip\n"), None);
    }
}
