//! `opendeskphone-image`: build and inspect Digium D-series firmware images.
//!
//! The Nix derivations call this for every image step; it is also usable by
//! hand on vendor files.  See `opendeskphone-image --help`.
//!
//! A flashable image from a built kernel and a rootfs directory:
//!
//! ```text
//! opendeskphone-image kernel-image --zimage zImage --dtb
//! bcm11189-digium-d60.dtb --model D60 --out d60-k.bin opendeskphone-image
//! image build --kernel d60-k.bin --rootfs rootfs/ --model D60 --version 3_0_0
//! \
//!     --node dev/console:c:5:1:600 --out 3_0_0_D60_firmware.eff
//! ```
//!
//! Installing an EFF from a shell on stock firmware (the same pipeline the
//! stock `curl_update_firmware` runs; it writes the bank that is not
//! running and switches to it, so run it only when that is intended):
//!
//! ```text
//! host$  scp -O 3_0_0_D60_firmware.eff root@PHONE:/tmp/fw.eff
//! phone# cd /tmp && rm -f upgpipe && mknod upgpipe p
//! phone# /app/bin/ota-file-unwrapper "$(cat /etc/phone-model)" /etc/otapassphrase < /tmp/fw.eff > upgpipe &
//! phone# upgrade-flash /tmp/upgpipe && sync && reboot
//! ```
//!
//! The stock web page (firmware upload) accepts the same file.
//!
//! The D80's image is an Android OTA package, made of an Android boot image
//! and a SquashFS image of the root file system (`mksquashfs` makes that);
//! see `d80.rs`:
//!
//! ```text
//! opendeskphone-image d80 boot-image --kernel zImage-dtb --out boot.img
//! opendeskphone-image d80 boot-image --kernel zImage-dtb --ramdisk recovery.cpio.xz --out recovery.img
//! opendeskphone-image d80 package --boot boot.img --recovery recovery.img --rootfs rootfs.squashfs \
//!     --update-binary update-binary --version 3_1_0 \\
//!     --key keys/d80-ota.key.pem --cert keys/d80-ota.x509.pem --out 3_1_0_D80_firmware.zip
//! opendeskphone-image eff pack 3_1_0_D80_firmware.zip 3_1_0_D80_firmware.eff --model D80 --version 3_1_0
//! ```

mod d80;
mod date;
mod kernel;

use clap::{Args, Parser, Subcommand, ValueEnum};
use eff::envelope::{self, EffHeader};
use eff::ota::{self, Ota};
use jffs2_image::{Compression, Entry, Kind, Options, Tree, read_image, write_image};
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};

type Res<T> = Result<T, String>;

/// Size of the root0/root1 partitions on every model.
const ROOT_PARTITION: usize = 0x2d0_0000;
const ERASE_BLOCK: usize = 0x20000;

#[derive(Parser)]
#[command(
    version,
    about = "Build and inspect Digium D-series firmware images (EFF, OTA, JFFS2, kernel)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The encrypted `.eff` envelope
    #[command(subcommand)]
    Eff(EffCmd),
    /// The OTA container inside an EFF (kernel + rootfs)
    #[command(subcommand)]
    Ota(OtaCmd),
    /// JFFS2 filesystem images
    #[command(subcommand)]
    Jffs2(Jffs2Cmd),
    /// zImage + DTB -> image for the k0/k1 slot, header patched for boot2
    KernelImage(KernelImage),
    /// Whole firmware images
    #[command(subcommand)]
    Image(ImageCmd),
    /// The D80's images: Android boot image, OTA package, voice processor firmware
    #[command(subcommand)]
    D80(D80Cmd),
}

#[derive(Subcommand)]
enum D80Cmd {
    /// zImage with its devicetree after it -> Android boot image for mmcblk0p1
    BootImage {
        /// The kernel: a zImage and then the D80's devicetree
        #[arg(long)]
        kernel: PathBuf,
        /// A ramdisk; default: an empty one, so that the kernel mounts the root file system itself
        #[arg(long)]
        ramdisk: Option<PathBuf>,
        /// Command line for the header (the D80's U-Boot passes its own, and the kernel uses the one built into it)
        #[arg(long, default_value = "")]
        cmdline: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the header of an Android boot image and check its id
    BootImageInfo { image: PathBuf },
    /// Boot image + recovery image + SquashFS root file system -> signed Android OTA package (zip)
    Package {
        /// Image made by `d80 boot-image`
        #[arg(long)]
        boot: PathBuf,
        /// The recovery image: one made by `d80 boot-image` with the recovery's ramdisk
        #[arg(long)]
        recovery: PathBuf,
        /// SquashFS image of the root file system
        #[arg(long)]
        rootfs: PathBuf,
        /// The `update-binary` of a stock OTA package (META-INF/com/google/android/update-binary)
        #[arg(long)]
        update_binary: PathBuf,
        /// Version text, e.g., 3_1_0
        #[arg(long)]
        version: String,
        /// RSA private key to sign the package with (PKCS #8, PEM)
        #[arg(long)]
        key: PathBuf,
        /// The key's X.509 certificate (PEM)
        #[arg(long)]
        cert: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Stock boot image -> the ZL38040's firmware as the mainline driver loads it (zl38040.fw)
    Zl38040Firmware {
        /// `boot.img` of a stock OTA package
        #[arg(long)]
        boot_image: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum EffCmd {
    /// Decrypt an EFF and write the OTA inside it (for the D80, an Android OTA package)
    Unpack { eff: PathBuf, ota: PathBuf },
    /// Encrypt an OTA (for the D80, an Android OTA package) into an EFF
    Pack {
        ota: PathBuf,
        eff: PathBuf,
        /// Phone model, e.g., D60 (stock refuses an EFF for another model)
        #[arg(long)]
        model: String,
        /// Version text, e.g., 3_0_0
        #[arg(long)]
        version: String,
        /// Eight-byte salt in hex; default: derived from the OTA, so the output is reproducible
        #[arg(long)]
        salt: Option<String>,
    },
    /// Print model, version, and the OTA header of an EFF (for the D80, the package size)
    Info { eff: PathBuf },
}

#[derive(Subcommand)]
enum OtaCmd {
    /// Check an OTA and write DIR/kernel.bin and DIR/rootfs.jffs2
    Unpack { ota: PathBuf, dir: PathBuf },
    /// Build an OTA from a kernel image and a rootfs image
    Pack {
        #[arg(long)]
        kernel: PathBuf,
        #[arg(long)]
        rootfs: PathBuf,
        /// Build-date text (at most 63 characters)
        #[arg(long)]
        date: String,
        /// File-name text, e.g., 3_0_0_D60_firmware.bin
        #[arg(long)]
        name: String,
        out: PathBuf,
    },
    /// Print the header of an OTA and check its CRCs
    Info { ota: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum Compr {
    /// zlib, then rtime, then stored (what the stock images use)
    Zlib,
    /// rtime only
    Rtime,
    None,
}

#[derive(Args)]
struct TreeOpts {
    /// Add a device node not present in the directory: PATH:c|b:MAJOR:MINOR:MODE (octal), e.g., dev/console:c:5:1:600
    #[arg(long = "node", value_name = "SPEC")]
    nodes: Vec<String>,
    /// Modification time for every file (seconds since 1970) [default: 0]
    #[arg(long, env = "SOURCE_DATE_EPOCH")]
    mtime: Option<u32>,
    /// Keep the directory's numeric owners instead of recording everything as root
    #[arg(long)]
    keep_owner: bool,
    #[arg(long, value_enum, default_value = "zlib")]
    compression: Compr,
}

#[derive(Subcommand)]
enum Jffs2Cmd {
    /// Build an image from a directory
    Build {
        dir: PathBuf,
        out: PathBuf,
        #[command(flatten)]
        tree: TreeOpts,
        /// Fill the last erase block with 0xff (the stock images do not)
        #[arg(long)]
        pad_last_block: bool,
    },
    /// List the files in an image
    List { image: PathBuf },
    /// Extract an image to a new directory (device nodes are skipped and reported)
    Extract { image: PathBuf, dir: PathBuf },
}

#[derive(Args)]
struct KernelImage {
    #[arg(long)]
    zimage: PathBuf,
    #[arg(long)]
    dtb: PathBuf,
    #[arg(long)]
    model: String,
    #[arg(long)]
    out: PathBuf,
    /// RAM address boot2 copies the image to and enters
    #[arg(long, value_parser = parse_u32, default_value = "0x02000000")]
    load: u32,
    /// Slot size; every bad block in a unit's slot lowers its real limit by 0x20000
    #[arg(long, value_parser = parse_u32, default_value = "0x200000")]
    slot: u32,
    /// Write the image even if boot2 could not load it from flash (RAM-boot use)
    #[arg(long)]
    force: bool,
}

#[derive(Subcommand)]
enum ImageCmd {
    /// Kernel image + rootfs -> flashable `.eff`
    Build {
        /// Image made by `kernel-image`
        #[arg(long)]
        kernel: PathBuf,
        /// Root filesystem directory (a JFFS2 image is made from it)
        #[arg(
            long,
            conflicts_with = "rootfs_image",
            required_unless_present = "rootfs_image"
        )]
        rootfs: Option<PathBuf>,
        /// Ready-made JFFS2 image instead of a directory
        #[arg(long)]
        rootfs_image: Option<PathBuf>,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "3_0_0")]
        version: String,
        #[arg(long)]
        out: PathBuf,
        /// Also write the unencrypted OTA here
        #[arg(long)]
        ota_out: Option<PathBuf>,
        /// Build-date text; default: the --mtime instant in the stock format
        #[arg(long)]
        date: Option<String>,
        #[command(flatten)]
        tree: TreeOpts,
    },
}

fn parse_u32(s: &str) -> Result<u32, String> {
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|e| e.to_string())
}

fn read(path: &Path) -> Res<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, data: &[u8]) -> Res<()> {
    std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}


fn load_tree(dir: &Path, o: &TreeOpts) -> Res<(Tree, Options)> {
    let mtime = o.mtime.unwrap_or(0);
    let mut tree = Tree::from_dir(dir, o.keep_owner, Some(mtime))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    for spec in &o.nodes {
        let parts: Vec<&str> = spec.split(':').collect();
        let err = || format!("bad --node {spec:?} (expected PATH:c|b:MAJOR:MINOR:MODE)");
        let [path, kind, major, minor, mode] = parts[..] else {
            return Err(err());
        };
        let major: u32 = major.parse().map_err(|_| err())?;
        let minor: u32 = minor.parse().map_err(|_| err())?;
        let mode = u32::from_str_radix(mode, 8).map_err(|_| err())?;
        let kind = match kind {
            "c" => Kind::CharDev { major, minor },
            "b" => Kind::BlockDev { major, minor },
            _ => return Err(err()),
        };
        tree.insert(
            path,
            Entry {
                kind,
                mode,
                uid: 0,
                gid: 0,
                mtime,
            },
        )
        .map_err(s)?;
    }
    let compression = match o.compression {
        Compr::Zlib => Compression::ZlibThenRtime,
        Compr::Rtime => Compression::Rtime,
        Compr::None => Compression::None,
    };
    Ok((
        tree,
        Options {
            compression,
            ..Options::default()
        },
    ))
}

fn describe_ota(o: &Ota) {
    let a = &o.header.app;
    println!("OTA: application update, all CRCs valid");
    println!(
        "  kernel {} bytes, CRC {:#010x}",
        a.kernel_size, a.kernel_crc
    );
    println!(
        "  rootfs {} bytes, CRC {:#010x}",
        a.rootfs_size, a.rootfs_crc
    );
    println!("  date {:?}", a.date());
    println!("  name {:?}", a.name());
}

fn deterministic_salt(ota: &[u8]) -> [u8; 8] {
    let d = Md5::digest(ota);
    d[..8].try_into().unwrap()
}

/// The model name an EFF carries: one `kernel-image` knows, or the D80.
fn eff_model(name: &str) -> Res<&'static str> {
    if envelope::is_d80(name) {
        return Ok("D80");
    }
    kernel::model(name).map(|m| m.name).map_err(|_| {
        format!("unknown model {name:?} (expected one of D40, D45, D50, D60, D62, D65, D70, D80)")
    })
}

/// What the payload of an EFF for `model` is.
fn payload_kind(model: &str) -> &'static str {
    if envelope::is_d80(model) {
        "Android OTA package"
    } else {
        "OTA"
    }
}

fn eff_cmd(cmd: EffCmd) -> Res<()> {
    match cmd {
        EffCmd::Unpack { eff, ota } => {
            let (h, payload) = envelope::unpack(&read(&eff)?).map_err(s)?;
            if !envelope::is_d80(&h.model) {
                Ota::parse(&payload).map_err(s)?;
            }
            write(&ota, &payload)?;
            println!(
                "Extracted {} {}: {} {} bytes",
                h.model,
                h.version,
                payload.len(),
                payload_kind(&h.model)
            );
        }
        EffCmd::Pack {
            ota,
            eff,
            model,
            version,
            salt,
        } => {
            let payload = read(&ota)?;
            let model = eff_model(&model)?;
            if !envelope::is_d80(model) {
                Ota::parse(&payload).map_err(s)?;
            }
            let header = EffHeader::new(model, &version).map_err(s)?;
            let salt = match salt {
                None => deterministic_salt(&payload),
                Some(h) => {
                    let ok = h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit());
                    if !ok {
                        return Err("--salt must be 16 hex digits".into());
                    }
                    let mut out = [0u8; 8];
                    for (i, b) in out.iter_mut().enumerate() {
                        *b = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap();
                    }
                    out
                }
            };
            let file = envelope::pack(&header, &payload, salt).map_err(s)?;
            write(&eff, &file)?;
            println!(
                "Created {} {}: {} ({} bytes)",
                header.model,
                header.version,
                eff.display(),
                file.len()
            );
        }
        EffCmd::Info { eff } => {
            let (h, payload) = envelope::unpack(&read(&eff)?).map_err(s)?;
            println!("EFF: model {} version {}", h.model, h.version);
            if envelope::is_d80(&h.model) {
                println!("{} (zip): {} bytes", payload_kind(&h.model), payload.len());
            } else {
                describe_ota(&Ota::parse(&payload).map_err(s)?);
            }
        }
    }
    Ok(())
}

fn ota_cmd(cmd: OtaCmd) -> Res<()> {
    match cmd {
        OtaCmd::Unpack { ota, dir } => {
            let data = read(&ota)?;
            let o = Ota::parse(&data).map_err(s)?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            write(&dir.join("kernel.bin"), o.kernel)?;
            write(&dir.join("rootfs.jffs2"), o.rootfs)?;
            describe_ota(&o);
        }
        OtaCmd::Pack {
            kernel,
            rootfs,
            date,
            name,
            out,
        } => {
            let data = ota::pack(&read(&kernel)?, &read(&rootfs)?, &date, &name).map_err(s)?;
            write(&out, &data)?;
            println!("Packed {} ({} bytes)", out.display(), data.len());
        }
        OtaCmd::Info { ota } => describe_ota(&Ota::parse(&read(&ota)?).map_err(s)?),
    }
    Ok(())
}

fn jffs2_cmd(cmd: Jffs2Cmd) -> Res<()> {
    match cmd {
        Jffs2Cmd::Build {
            dir,
            out,
            tree,
            pad_last_block,
        } => {
            let (t, mut opts) = load_tree(&dir, &tree)?;
            opts.pad_last_block = pad_last_block;
            let image = write_image(&t, &opts).map_err(s)?;
            write(&out, &image)?;
            println!(
                "{}: {} entries, {} bytes",
                out.display(),
                t.entries.len(),
                image.len()
            );
        }
        Jffs2Cmd::List { image } => {
            let (t, stats) = read_image(&read(&image)?, ERASE_BLOCK).map_err(s)?;
            for (path, e) in &t.entries {
                let mode = e.file_type_bits() | e.mode;
                let own = format!("{}:{}", e.uid, e.gid);
                match &e.kind {
                    Kind::File(d) => println!("{mode:06o} {own:>9} {:>9} {path}", d.len()),
                    Kind::Symlink(target) => {
                        println!(
                            "{mode:06o} {own:>9} {:>9} {path} -> {}",
                            target.len(),
                            String::from_utf8_lossy(target)
                        )
                    }
                    Kind::CharDev { major, minor } | Kind::BlockDev { major, minor } => {
                        println!(
                            "{mode:06o} {own:>9} {:>9} {path}",
                            format!("{major},{minor}")
                        )
                    }
                    _ => println!("{mode:06o} {own:>9} {:>9} {path}", "-"),
                }
            }
            eprintln!("{} entries; {stats:?}", t.entries.len());
        }
        Jffs2Cmd::Extract { image, dir } => {
            if dir.exists() {
                return Err(format!("{} already exists", dir.display()));
            }
            let (t, _) = read_image(&read(&image)?, ERASE_BLOCK).map_err(s)?;
            let skipped = t.extract(&dir).map_err(s)?;
            println!(
                "Extracted {} entries to {}",
                t.entries.len() - skipped.len(),
                dir.display()
            );
            for p in skipped {
                println!("skipped (device node, FIFO, or socket): {p}");
            }
        }
    }
    Ok(())
}

fn kernel_image(k: KernelImage) -> Res<()> {
    let m = kernel::model(&k.model)?;
    let built = kernel::build(
        &read(&k.zimage)?,
        &read(&k.dtb)?,
        m,
        k.load,
        k.slot,
        k.force,
    )?;
    write(&k.out, &built.image)?;
    println!("{}: {built}", k.out.display());
    Ok(())
}

fn image_cmd(cmd: ImageCmd) -> Res<()> {
    let ImageCmd::Build {
        kernel,
        rootfs,
        rootfs_image,
        model,
        version,
        out,
        ota_out,
        date,
        tree,
    } = cmd;
    let m = kernel::model(&model)?;
    let kimg = read(&kernel)?;
    kernel::check(&kimg, m, kernel::DEFAULT_SLOT)
        .map_err(|e| format!("{}: {e}", kernel.display()))?;
    let rimg = match (rootfs, rootfs_image) {
        (Some(dir), _) => {
            let (t, opts) = load_tree(&dir, &tree)?;
            if t.get("sbin/init").is_none() {
                return Err(format!(
                    "{}: no /sbin/init; boot2's command line has no init=, so the kernel runs /sbin/init",
                    dir.display()
                ));
            }
            write_image(&t, &opts).map_err(s)?
        }
        (None, Some(img)) => {
            let data = read(&img)?;
            read_image(&data, ERASE_BLOCK).map_err(|e| format!("{}: {e}", img.display()))?;
            data
        }
        (None, None) => unreachable!("clap requires one"),
    };
    if rimg.len() > ROOT_PARTITION {
        return Err(format!(
            "rootfs image is {} bytes; the root partition holds {ROOT_PARTITION}",
            rimg.len()
        ));
    }
    let date = match date {
        Some(d) => d,
        None => date::ctime_utc(tree.mtime.unwrap_or(0).into()),
    };
    let name = format!("{version}_{}_firmware.bin", m.name);
    let payload = ota::pack(&kimg, &rimg, &date, &name).map_err(s)?;
    let header = EffHeader::new(m.name, &version).map_err(s)?;
    let file = envelope::pack(&header, &payload, deterministic_salt(&payload)).map_err(s)?;
    // Read back what a phone would: decrypt, then every CRC.
    let (h2, back) = envelope::unpack(&file).map_err(s)?;
    if h2 != header || back != payload {
        return Err("internal error: EFF does not decrypt to its input".into());
    }
    if let Some(p) = ota_out {
        write(&p, &payload)?;
    }
    write(&out, &file)?;
    println!(
        "{}: {} {} ({} bytes): kernel {} bytes ({} spare in the 2 MiB slot), rootfs {} bytes ({} blocks of {})",
        out.display(),
        m.name,
        version,
        file.len(),
        kimg.len(),
        kernel::DEFAULT_SLOT as usize - kimg.len(),
        rimg.len(),
        rimg.len().div_ceil(ERASE_BLOCK),
        ROOT_PARTITION / ERASE_BLOCK
    );
    Ok(())
}

fn d80_cmd(cmd: D80Cmd) -> Res<()> {
    match cmd {
        D80Cmd::BootImage { kernel, ramdisk, cmdline, out } => {
            let ramdisk = match ramdisk {
                Some(p) => read(&p)?,
                None => d80::empty_ramdisk(),
            };
            let image = d80::BootImage::new(read(&kernel)?, ramdisk, &cmdline);
            let data = image.build()?;
            write(&out, &data)?;
            println!(
                "{}: kernel {} bytes, ramdisk {} bytes, {} bytes in all ({} spare in the 8 MiB partition)",
                out.display(),
                image.kernel.len(),
                image.ramdisk.len(),
                data.len(),
                d80::BOOT_PARTITION - data.len()
            );
        }
        D80Cmd::BootImageInfo { image } => {
            let b = d80::BootImage::parse(&read(&image)?)?;
            println!("Android boot image, header version 0, id valid");
            println!("  kernel {} bytes, loaded at {:#010x}", b.kernel.len(), b.kernel_addr);
            println!("  ramdisk {} bytes, loaded at {:#010x}", b.ramdisk.len(), b.ramdisk_addr);
            println!("  second-stage address {:#010x}, tags address {:#010x}", b.second_addr, b.tags_addr);
            println!("  command line {:?}", b.cmdline);
            for p in b.problems() {
                println!("  not as this tool builds one: {p}");
            }
        }
        D80Cmd::Package { boot, recovery, rootfs, update_binary, version, key, cert, out } => {
            let (boot, recovery, rootfs) = (read(&boot)?, read(&recovery)?, read(&rootfs)?);
            let text = |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
            let data = d80::package(&boot, &recovery, &rootfs, &read(&update_binary)?, &version, &text(&key)?, &text(&cert)?)?;
            write(&out, &data)?;
            println!(
                "{}: D80 {version} ({} bytes, signed): boot image {} bytes, recovery image {} bytes, root file system {} bytes",
                out.display(),
                data.len(),
                boot.len(),
                recovery.len(),
                rootfs.len()
            );
        }
        D80Cmd::Zl38040Firmware { boot_image, out } => {
            let fw = d80::zl38040_firmware(&read(&boot_image)?)?;
            write(&out, &fw)?;
            println!("{}: {} bytes", out.display(), fw.len());
        }
    }
    Ok(())
}

fn main() {
    let res = match Cli::parse().cmd {
        Cmd::Eff(c) => eff_cmd(c),
        Cmd::Ota(c) => ota_cmd(c),
        Cmd::Jffs2(c) => jffs2_cmd(c),
        Cmd::KernelImage(k) => kernel_image(k),
        Cmd::Image(c) => image_cmd(c),
        Cmd::D80(c) => d80_cmd(c),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
