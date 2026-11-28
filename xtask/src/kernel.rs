//! Kernel port maintenance.
//!
//! The port is kept in the repository as:
//!
//! - `kernel/patches/NNNN-<group>.patch`: changes to files that exist in
//!   Linux v7.3.5, one patch per subsystem group, applied with `patch -p1`;
//! - `kernel/files/`: files the port adds, at their kernel-tree paths;
//! - `kernel/configs/`: the defconfigs (they belong in `arch/arm/configs/`).
//!
//! `kernel-sync` regenerates all three from a git checkout of v7.3.5 whose
//! working tree holds the port.  The build itself is the Nix derivation in
//! `nix/kernel.nix`; `kernel/README.md` says how to get a tree to edit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{Result, repo_root};

const CONFIG_DIR: &str = "arch/arm/configs/";

/// Path prefix to patch group, first match wins.  Paths that match nothing
/// are grouped by their first two components.
const GROUPS: &[(&str, &str)] = &[
    ("Documentation/", "docs"),
    ("arch/arm/", "arch-arm"),
    ("drivers/char/hw_random/", "hwrng"),
    ("drivers/clk/", "clk"),
    ("drivers/dma/", "dma"),
    ("include/linux/dma/", "dma"),
    ("drivers/gpio/", "gpio"),
    ("drivers/gpu/", "drm"),
    ("drivers/input/", "input"),
    ("drivers/irqchip/", "irqchip"),
    ("drivers/leds/", "leds"),
    ("drivers/mtd/", "mtd"),
    ("drivers/net/", "net"),
    ("include/linux/brcmphy.h", "net"),
    ("drivers/remoteproc/", "remoteproc"),
    ("drivers/video/", "backlight"),
    ("drivers/watchdog/", "watchdog"),
    ("sound/", "sound"),
];

fn group_of(path: &str) -> String {
    for (prefix, group) in GROUPS {
        if path.starts_with(prefix) {
            return (*group).to_string();
        }
    }
    path.split('/').take(2).collect::<Vec<_>>().join("-")
}

fn git(tree: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(tree)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .stderr(Stdio::inherit())
        .output()?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), out.status).into());
    }
    Ok(out.stdout)
}

fn lines(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(std::str::from_utf8(bytes)?
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn kernel_dir() -> PathBuf {
    repo_root().join("kernel")
}

#[derive(Debug, clap::Args)]
pub struct KernelSync {
    /// The kernel working tree
    tree: PathBuf,
}

fn remove_dir_if_exists(p: &Path) -> Result<()> {
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to).map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
    Ok(())
}

/// All regular files under `dir`, as paths relative to it, sorted.
pub fn sync(args: KernelSync) -> Result<()> {
    let tree = args.tree.as_path();
    let kdir = kernel_dir();

    let modified = lines(&git(tree, &["diff", "--name-only", "--no-renames", "HEAD"])?)?;
    let added = lines(&git(tree, &["ls-files", "--others", "--exclude-standard"])?)?;

    // Patches, one per group.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in &modified {
        if !tree.join(path).exists() {
            return Err(format!("{path} is deleted in the tree; deletions are not supported").into());
        }
        groups.entry(group_of(path)).or_default().push(path.clone());
    }
    let patches = kdir.join("patches");
    remove_dir_if_exists(&patches)?;
    fs::create_dir_all(&patches)?;
    for (n, (group, files)) in groups.iter().enumerate() {
        let mut cmd = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--full-index",
            "HEAD",
            "--",
        ];
        cmd.extend(files.iter().map(String::as_str));
        let diff = git(tree, &cmd)?;
        let name = format!("{:04}-{group}.patch", n + 1);
        fs::write(patches.join(&name), diff)?;
        println!("patches/{name}: {} file(s)", files.len());
    }

    // New files; the defconfigs go to kernel/configs.
    let files_dir = kdir.join("files");
    let configs_dir = kdir.join("configs");
    remove_dir_if_exists(&files_dir)?;
    remove_dir_if_exists(&configs_dir)?;
    let (mut nfiles, mut nconfigs) = (0, 0);
    for path in &added {
        let dest = match path.strip_prefix(CONFIG_DIR) {
            Some(name) => {
                nconfigs += 1;
                configs_dir.join(name)
            }
            None => {
                nfiles += 1;
                files_dir.join(path)
            }
        };
        copy_file(&tree.join(path), &dest)?;
    }
    println!("files/: {nfiles} file(s); configs/: {nconfigs} file(s)");
    Ok(())
}
