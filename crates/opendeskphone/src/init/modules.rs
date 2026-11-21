//! Kernel module loading (`finit_module`), with dependency resolution from
//! `modules.dep` when the image carries one.
//!
//! The kernel slot is too small for every driver, so display (old family),
//! audio, DMA, and the DSP driver live in the rootfs.  The D80's kernel has
//! room, and has its sound and Bluetooth drivers as modules all the same;
//! its image carries `modules.dep`.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use log::{debug, info, warn};

use crate::model::{Family, Model};
use crate::sys;

pub const MODULES_ROOT: &str = "/lib/modules";

/// Modules to load at boot, in order.  Dependencies come from `modules.dep`;
/// without it, every name here is tried in this order, so the lists spell
/// out the dependency chains too.  A file `/etc/opendeskphone/modules-<family>`
/// (one name per line, `#` comments) replaces the list for that family.
pub fn boot_list(family: Family) -> Vec<String> {
    let key = match family {
        Family::Old => "old",
        Family::New => "new",
        Family::Touch => "touch",
    };
    if let Ok(text) = std::fs::read_to_string(format!("/etc/opendeskphone/modules-{key}")) {
        return parse_list(&text);
    }
    let mut list: Vec<String> = Vec::new();
    if family == Family::Old {
        list.extend(OLD_FAMILY_DISPLAY.iter().map(|s| s.to_string()));
    }
    // The D80 is not a BCMRING phone: none of those modules is its.
    let rest = if family == Family::Touch { TOUCH } else { ALL_MODELS };
    list.extend(rest.iter().map(|s| s.to_string()));
    list
}

/// The D80: its sound card (the SoC's audio port and serial audio
/// interface, the ZL38040 voice processor, and the card that joins them).
/// What each needs loaded before it comes from the image's `modules.dep`.
/// The voice processor's driver loads `zl38040.fw` as it starts, so the
/// firmware path is set first.
const TOUCH: &[&str] = &["snd-soc-imx-audmux", "snd-soc-fsl-ssi", "snd-soc-zl38040", "snd-soc-simple-card"];

/// The D80's Bluetooth: the AP6212 module's controller on UART4, which
/// `hci_uart` drives with the Broadcom protocol (and loads its patch file,
/// `brcm/BCM4343A0.hcd`).  As for [`TOUCH`], `modules.dep` names the rest.
const TOUCH_BLUETOOTH: &[&str] = &["rfcomm", "hci_uart"];

/// Old family only, first, so the SPI display comes up early.
const OLD_FAMILY_DISPLAY: &[&str] = &["spi-pl022", "lcd", "drm_shmem_helper", "digium-gray-lcd"];

/// Every model of the old and new families, in dependency order (see `kernel/README.md`).
const ALL_MODELS: &[&str] = &[
    "dw_dmac_core",
    "dw_dmac",
    "soundcore",
    "snd",
    "snd-timer",
    "snd-pcm",
    "snd-pcm-dmaengine",
    "snd-soc-core",
    "snd-soc-simple-amplifier",
    "snd-soc-bcmring-apm",
    "snd-soc-bcmring-digium-card",
    "bcmring_vpm",
];

/// Where the image keeps each family's DSP firmware.
pub fn firmware_dir(family: Family) -> &'static str {
    match family {
        Family::Old => "/lib/firmware/old-family",
        Family::New => "/lib/firmware/new-family",
        Family::Touch => "/lib/firmware",
    }
}

/// Point the kernel's firmware loader at this family's directory.  Must run
/// before `bcmring_vpm` loads, or the DSP firmware is not found.
pub fn set_firmware_path(family: Family) {
    const PARAM: &str = "/sys/module/firmware_class/parameters/path";
    let dir = firmware_dir(family);
    if let Err(e) = std::fs::write(PARAM, dir) {
        warn!("firmware path {dir}: {e}");
    }
}

pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Module names treat `-` and `_` as the same character.
pub fn normalize(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.split(".ko").next().unwrap_or(base);
    stem.replace('-', "_")
}

/// Index of the modules available under a root directory.
#[derive(Default)]
pub struct Index {
    /// Normalized name to file path.
    files: HashMap<String, PathBuf>,
    /// Normalized name to its direct dependencies (normalized names), in the
    /// order `modules.dep` lists them (nearest dependency first).
    deps: HashMap<String, Vec<String>>,
}

impl Index {
    /// Scan `root` (normally `/lib/modules`): every `*.ko*` file below it, and
    /// any `modules.dep` found on the way.
    pub fn scan(root: &Path) -> Self {
        let mut index = Index::default();
        let mut stack = vec![root.to_path_buf()];
        let mut depth_guard = 0;
        while let Some(dir) = stack.pop() {
            depth_guard += 1;
            if depth_guard > 10_000 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "modules.dep" {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        index.add_dep_file(&text);
                    }
                } else if name.ends_with(".ko") || name.contains(".ko.") {
                    index.files.insert(normalize(&name), path);
                }
            }
        }
        index
    }

    /// Parse `modules.dep` text: `path/to/a.ko: path/to/b.ko path/to/c.ko`.
    pub fn add_dep_file(&mut self, text: &str) {
        for line in text.lines() {
            let Some((module, deps)) = line.split_once(':') else { continue };
            let deps = deps.split_ascii_whitespace().map(normalize).collect();
            self.deps.insert(normalize(module.trim()), deps);
        }
    }

    pub fn path(&self, name: &str) -> Option<&Path> {
        self.files.get(&normalize(name)).map(PathBuf::as_path)
    }

    /// Load order for `name`: its dependencies (deepest first), then itself.
    pub fn load_order(&self, name: &str) -> Vec<String> {
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        self.visit(&normalize(name), &mut seen, &mut order);
        order
    }

    fn visit(&self, name: &str, seen: &mut HashSet<String>, order: &mut Vec<String>) {
        if !seen.insert(name.to_string()) {
            return;
        }
        if let Some(deps) = self.deps.get(name) {
            // modules.dep lists nearest first; load in reverse.
            for dep in deps.iter().rev() {
                self.visit(dep, seen, order);
            }
        }
        order.push(name.to_string());
    }
}

/// Names of the modules the kernel has loaded.
pub fn loaded() -> HashSet<String> {
    std::fs::read_to_string("/proc/modules")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_ascii_whitespace().next())
        .map(normalize)
        .collect()
}

/// Load one module file.  "Already loaded" counts as success.
pub fn insmod(path: &Path, params: &str) -> io::Result<()> {
    let file = File::open(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let flags = if name.ends_with(".ko") { 0 } else { sys::MODULE_INIT_COMPRESSED_FILE };
    match sys::finit_module(file.as_raw_fd(), params, flags) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Load `name` and its dependencies.  Returns the names newly loaded.
pub fn modprobe(index: &Index, name: &str, params: &str) -> io::Result<Vec<String>> {
    let already = loaded();
    let target = normalize(name);
    let mut done = Vec::new();
    for module in index.load_order(name) {
        if already.contains(&module) {
            continue;
        }
        let Some(path) = index.path(&module) else {
            if module == target {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("module {name} not found")));
            }
            // A dependency with no file is built in or missing; the kernel
            // will say which when the dependent module loads.
            debug!("module {module} (dependency of {name}): no file");
            continue;
        };
        insmod(path, if module == target { params } else { "" })?;
        done.push(module);
    }
    Ok(done)
}

/// Modules one model needs beyond its family's list: Bluetooth on the D65
/// and the D80.
pub fn model_list(model: Model) -> Vec<String> {
    match model {
        Model::D65 => opendeskphone_bt::KERNEL_MODULES.iter().map(|s| s.to_string()).collect(),
        Model::D80 => TOUCH_BLUETOOTH.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Load the boot list for a model: its family's modules, then its own.
/// Nothing here is fatal: a missing or failing module is logged and the
/// boot goes on.
pub fn load_boot_modules(model: Model) {
    let family = model.family();
    set_firmware_path(family);
    let index = Index::scan(Path::new(MODULES_ROOT));
    let have_deps = !index.deps.is_empty();
    for name in boot_list(family).into_iter().chain(model_list(model)) {
        if index.path(&name).is_none() {
            // Without modules.dep the list carries dependency names that may
            // be built in; say so quietly.
            if have_deps {
                warn!("module {name}: not in {MODULES_ROOT}");
            } else {
                debug!("module {name}: not in {MODULES_ROOT}");
            }
            continue;
        }
        match modprobe(&index, &name, "") {
            Ok(loaded) if loaded.is_empty() => {}
            Ok(loaded) => info!("loaded {}", loaded.join(", ")),
            Err(e) => warn!("module {name}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(normalize("kernel/drivers/dma/dw/dw_dmac.ko"), "dw_dmac");
        assert_eq!(normalize("snd-soc-bcmring-apm.ko.xz"), "snd_soc_bcmring_apm");
        assert_eq!(normalize("spi-pl022"), "spi_pl022");
    }

    #[test]
    fn dependency_order() {
        let mut index = Index::default();
        index.add_dep_file(
            "kernel/sound/soc/bcm/snd-soc-bcmring-apm.ko: kernel/sound/soc/snd-soc-core.ko kernel/sound/core/snd-pcm.ko kernel/sound/core/snd.ko\n\
             kernel/sound/soc/snd-soc-core.ko: kernel/sound/core/snd-pcm.ko kernel/sound/core/snd.ko\n\
             kernel/sound/core/snd-pcm.ko: kernel/sound/core/snd.ko\n\
             kernel/sound/core/snd.ko:\n",
        );
        assert_eq!(
            index.load_order("snd-soc-bcmring-apm"),
            ["snd", "snd_pcm", "snd_soc_core", "snd_soc_bcmring_apm"]
        );
        assert_eq!(index.load_order("unknown"), ["unknown"]);
    }

    #[test]
    fn scan_finds_files() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("7.3.0/kernel/drivers");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("dw_dmac.ko"), b"").unwrap();
        std::fs::write(sub.join("dw_dmac_core.ko"), b"").unwrap();
        std::fs::write(
            dir.path().join("7.3.0/modules.dep"),
            "kernel/drivers/dw_dmac.ko: kernel/drivers/dw_dmac_core.ko\nkernel/drivers/dw_dmac_core.ko:\n",
        )
        .unwrap();
        let index = Index::scan(dir.path());
        assert!(index.path("dw-dmac").is_some());
        assert_eq!(index.load_order("dw_dmac"), ["dw_dmac_core", "dw_dmac"]);
    }

    #[test]
    fn list_file() {
        assert_eq!(parse_list("# comment\ndw_dmac\n\n  bcmring_vpm # dsp\n"), ["dw_dmac", "bcmring_vpm"]);
    }

    /// The D80's lists are its own, and the dependencies of the built
    /// kernel's `modules.dep` put each module after what it needs.
    #[test]
    fn the_d80_loads_its_sound_card_and_bluetooth() {
        let family: Vec<String> = TOUCH.iter().map(|s| s.to_string()).collect();
        assert!(!family.iter().any(|m| m.contains("bcmring") || m.contains("dw_dmac")));
        assert!(family.contains(&"snd-soc-zl38040".to_string()));
        assert_eq!(model_list(Model::D80), ["rfcomm", "hci_uart"]);
        let mut index = Index::default();
        index.add_dep_file(
            "kernel/drivers/bluetooth/hci_uart.ko: kernel/drivers/bluetooth/btbcm.ko kernel/net/bluetooth/bluetooth.ko kernel/lib/crypto/libaes.ko kernel/crypto/ecdh_generic.ko kernel/crypto/ecc.ko\n\
             kernel/drivers/bluetooth/btbcm.ko: kernel/net/bluetooth/bluetooth.ko kernel/lib/crypto/libaes.ko kernel/crypto/ecdh_generic.ko kernel/crypto/ecc.ko\n\
             kernel/net/bluetooth/bluetooth.ko: kernel/lib/crypto/libaes.ko kernel/crypto/ecdh_generic.ko kernel/crypto/ecc.ko\n\
             kernel/crypto/ecdh_generic.ko: kernel/crypto/ecc.ko\n",
        );
        assert_eq!(index.load_order("hci_uart"), ["ecc", "ecdh_generic", "libaes", "bluetooth", "btbcm", "hci_uart"]);
    }

    /// The D60 shares the D65's family but has no Bluetooth.
    #[test]
    fn the_d60_loads_no_bluetooth_modules() {
        assert!(model_list(Model::D60).is_empty());
    }
}
