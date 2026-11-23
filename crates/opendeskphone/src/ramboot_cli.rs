//! `dphone ramboot IMAGE.eff`: start an image's kernel and root file system
//! from RAM, without installing it.
//!
//! For trying a kernel that might not boot: nothing is written to the
//! kernel or root partitions or to `ctrl`, so a power cycle always brings
//! the installed firmware back, and so does the hardware watchdog when the
//! new kernel hangs (it keeps running across the jump).
//!
//! The work is in three places:
//!
//! - `opendeskphone_update::ramboot` reads and verifies the image and lays it out
//!   (see there for the design);
//! - this command gathers the facts about the running phone, hands the
//!   layout to the kernel with `kexec_load(2)`, and asks PID 1 to go;
//! - the init role shuts the system down in order (main role, every other
//!   process, nvdata unmounted) and makes the `reboot(2)` call that jumps.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use opendeskphone_update::ramboot::{self, Options, Plan};

use crate::sys;

/// Start the kernel, devicetree, and root file system of an Open Desk Phone
/// image from RAM
///
/// Nothing is written to the kernel or root partitions or to 'ctrl': a
/// reboot, a power cycle, or a kernel that hangs (the watchdog resets the
/// phone) brings the installed firmware back.  /nvdata is the phone's own,
/// mounted as always, so the started system has the same settings,
/// address, and SSH keys.  A pending trial boot is neither counted nor
/// confirmed by a system started this way.
#[derive(Debug, clap::Args)]
pub struct Ramboot {
    /// The file 'dphone update install' installs; '-' reads it from
    /// standard input (saves the copy in /tmp)
    image: PathBuf,
    /// The started system resets the phone after this long unless
    /// /run/opendeskphone/stay exists; 0 for no limit
    #[arg(long, value_name = "SECONDS", default_value_t = ramboot::DEFAULT_MAX_UPTIME)]
    max_uptime: u64,
    /// Extra kernel arguments (e.g., amba_pl011.console_output=1)
    #[arg(long, value_name = "ARGUMENTS", default_value = "", hide_default_value = true, allow_hyphen_values = true)]
    append: String,
    /// Verify the image and print what would be loaded and passed to the
    /// kernel, then stop
    #[arg(long)]
    dry_run: bool,
}

impl Ramboot {
    fn options(&self) -> Options {
        Options { max_uptime: (self.max_uptime != 0).then_some(self.max_uptime), append: self.append.clone() }
    }
}

pub fn run(args: &Ramboot) -> i32 {
    match start(args) {
        Ok(()) => 0,
        Err(e) => {
            crate::errln!("dphone ramboot: {e}");
            1
        }
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// Why the jump cannot be made from this system, if it cannot.
fn not_ready() -> Option<String> {
    if !std::path::Path::new(sys::KEXEC_LOADED).exists() {
        return Some(
            "the running kernel has no kexec; install an image whose kernel has it once, with 'dphone update'".into(),
        );
    }
    // PID 1 must be our init role with the watchdog armed: it does the
    // orderly shutdown, and the watchdog is what ends a kernel that hangs.
    let init_has_watchdog = std::fs::read_dir("/proc/1/fd").is_ok_and(|dir| {
        dir.flatten().any(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.starts_with("/dev/watchdog")))
    });
    if !init_has_watchdog {
        return Some(
            "the init role does not hold the watchdog, so a kernel that hangs would stay hung until a power cycle".into(),
        );
    }
    None
}

fn start(args: &Ramboot) -> Result<(), String> {
    let model: opendeskphone_state::Model = crate::model::detect()
        .map(|d| d.model.into())
        .ok_or("model unknown (no digium_board= and no devicetree match)")?;
    let ready = not_ready();
    if let (Some(why), false) = (&ready, args.dry_run) {
        return Err(why.clone());
    }

    let input: Box<dyn Read> = if args.image == Path::new("-") {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(File::open(&args.image).map_err(|e| format!("{}: {e}", args.image.display()))?)
    };
    let opened = ramboot::open(BufReader::with_capacity(64 * 1024, input), model).map_err(|e| e.to_string())?;
    let meminfo = read("/proc/meminfo")?;
    let mem = |key: &str| ramboot::meminfo(&meminfo, key).ok_or(format!("/proc/meminfo: no {key}"));
    opened.check_memory(mem("MemAvailable")?, mem("MemTotal")?).map_err(|e| e.to_string())?;
    crate::outln!(
        "image: {model} {}, kernel {} bytes, root file system {} bytes; verifying",
        opened.version(),
        opened.kernel_size(),
        opened.rootfs_size()
    );
    let image = opened.load().map_err(|e| e.to_string())?;
    let ram = ramboot::system_ram(&read("/proc/iomem")?).map_err(|e| e.to_string())?;
    let plan = ramboot::plan(&image, ram, &read("/proc/cmdline")?, &args.options()).map_err(|e| e.to_string())?;
    describe(&plan, &image);

    if args.dry_run {
        match ready {
            Some(why) => crate::outln!("dry run: nothing loaded.  Not startable from this system: {why}"),
            None => crate::outln!("dry run: nothing loaded"),
        }
        return Ok(());
    }

    let segments: Vec<sys::KexecSegment> = plan
        .segments(&image)
        .iter()
        .map(|s| sys::KexecSegment {
            buf: s.data.as_ptr() as *const libc::c_void,
            bufsz: s.data.len(),
            mem: s.mem as usize,
            memsz: s.memsz as usize,
        })
        .collect();
    // SAFETY: every segment points into `plan` or `image`, both alive
    // until after the call; the kernel copies the data during it.
    unsafe { sys::kexec_load(plan.entry as usize, &segments) }.map_err(|e| {
        let hint = match e.raw_os_error() {
            Some(libc::EPERM) => " (not root, or kexec is disabled)",
            Some(libc::ENOMEM) => " (not enough free memory)",
            Some(libc::EADDRNOTAVAIL) | Some(libc::EINVAL) => " (the kernel does not accept this layout)",
            Some(libc::EBUSY) => " (another image is being loaded)",
            _ => "",
        };
        format!("kexec_load: {e}{hint}")
    })?;
    drop(segments);
    drop(image);
    if read(sys::KEXEC_LOADED)?.trim() != "1" {
        return Err("the kernel did not keep the image".into());
    }

    crate::outln!(
        "loaded.  Starting it now: this session ends, and the phone answers again in about a minute.{}",
        match args.options().max_uptime {
            Some(secs) => format!(
                "  It returns to the installed firmware after {secs} s unless /run/opendeskphone/stay exists there, \
                 and at any 'dphone reboot' or power cycle."
            ),
            None => "  It returns to the installed firmware at 'dphone reboot' or a power cycle.".into(),
        }
    );
    crate::out::flush();
    if let Err(e) = sys::kill(1, sys::kexec_signal()) {
        // Do not leave an image behind that nobody asked to start.
        sys::kexec_unload();
        return Err(format!("cannot signal PID 1: {e}"));
    }
    Ok(())
}

fn describe(plan: &Plan, image: &ramboot::Image) {
    for s in plan.segments(image) {
        crate::outln!("  {:<17} {:#010x}..{:#010x} ({} bytes)", s.name, s.mem, s.mem as u64 + s.memsz as u64, s.data.len());
    }
    crate::outln!(
        "  memory for the started kernel: {:#010x}..{:#010x} ({} MiB)",
        plan.mem_start,
        plan.root_at,
        plan.mem_size() >> 20
    );
    crate::outln!("  entry {:#010x}, tag list at {:#010x}", plan.entry, plan.atags_at);
    crate::outln!("  command line: {} {}", image.dtb_bootargs, plan.cmdline);
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        ramboot: Ramboot,
    }

    fn args(words: &[&str]) -> Result<Ramboot, clap::Error> {
        Cli::try_parse_from(std::iter::once(&"ramboot").chain(words)).map(|c| c.ramboot)
    }

    #[test]
    fn arguments() {
        let a = args(&["/tmp/fw.eff"]).unwrap();
        assert_eq!(a.options(), Options::default());

        let a = args(&["--max-uptime", "120", "fw.eff", "--append", "panic=30 loglevel=7", "--dry-run"]).unwrap();
        assert_eq!(a.options(), Options { max_uptime: Some(120), append: "panic=30 loglevel=7".into() });

        // No limit.
        let a = args(&["-", "--max-uptime", "0"]).unwrap();
        assert_eq!(a.options().max_uptime, None);
    }

    /// A file that is not there: an error, not a usage problem.  (On a
    /// workstation the model is unknown first; either way it is 1.)
    #[test]
    fn a_failure_is_not_a_usage_problem() {
        assert_eq!(run(&args(&["/nonexistent/image.eff", "--dry-run"]).unwrap()), 1);
    }
}
