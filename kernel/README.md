# Kernel

This directory holds the Open Desk Phone kernel: mainline Linux 7.3 with two ports.  One is to the Broadcom BCMRING family of system-on-chip processors, which the Digium D40, D45, D50, D60, D62, D65, and D70 phones are built on.  The other is to the Digium D80, which is built on the NXP i.MX 6DualLite, a processor that Linux 7.3 already supports.  The Linux source itself is not in this repository.  The build downloads the Linux 7.3 release and applies what is here.

Both ports are in one source tree, and the build makes two kernels of it: one for the seven BCMRING phones, and one for the D80.  Except for [The D80's kernel](#the-d80s-kernel), the sections below describe the kernel of the BCMRING phones.

## What is in this directory

| Path | Contents |
| --- | --- |
| `patches/` | Changes to files that exist in Linux 7.3, with one patch file for each kernel subsystem.  The build applies them in the order of their file names with `patch -p1`. |
| `files/` | Files that the port adds (drivers, device trees, device tree binding documents, and headers), stored at the paths they have in the kernel tree.  The build copies them into the patched tree. |
| `configs/` | Kernel configurations.  The build copies them to `arch/arm/configs/`.  The firmware of the BCMRING phones uses `bcmring_tiny_defconfig`.  `bcmring_defconfig` is a larger configuration that the firmware build does not use.  The D80's kernel uses `d80_defconfig`. |

## Building

Nix builds the kernel, and `nix/kernel.nix` is the recipe.

| Command | Result |
| --- | --- |
| `nix build .#kernel` | The compressed kernel (`zImage`), one device tree for each model (`dtbs/`), the loadable modules (`lib/modules/7.3.0/`), and the full configuration that was used (`config`). |
| `nix build .#kernel-d60` | `kernel.bin`, the file that is written to the kernel partition of a D60.  There is one such output for each model. |
| `nix build .#kernel-d80` | The D80's kernel.  See [The D80's kernel](#the-d80s-kernel). |
| `nix build .#boot-d80` | The D80's Android boot image, which is written to the first partition of its eMMC. |
| `nix build .#recovery-d80` | The D80's recovery image, which is written to the second partition of its eMMC: the same kernel, with the recovery program in its ramdisk. |
| `nix build .#kernel-source` | The Linux 7.3 tree with both ports applied and nothing built. |
| `nix build .#eff-d60` | The complete firmware image for a D60, which contains `kernel.bin`. |

The compiled kernel is the same for all seven BCMRING models, and only the device tree differs.  Any change under `kernel/` makes Nix build both kernels again, because a Nix build cannot reuse object files from an earlier build.

## Constraints to know about

- **The kernel partition is 2 MiB.**  The kernel and the device tree together must fit in it, and the build fails if they do not.  This is why several drivers (the grayscale display, the sound drivers, the DMA controller driver, and the voice processor driver) are built as modules and stored in the root file system.  The firmware loads them when it starts, from the list in `crates/opendeskphone/src/init/modules.rs`.
- **The kernel image carries the early-boot stage.**  The build puts one program, `crates/opendeskphone-early`, into the kernel as its initramfs (`CONFIG_INITRAMFS_SOURCE`, from `crates/opendeskphone-early/initramfs.list`).  It is the first program the kernel runs: it counts trial boots after a firmware update, selects the previous firmware bank when the new one does not come up, and otherwise mounts the root file system named by `root=` and runs its `/sbin/init`.  The kernel does not mount the root file system by itself any more.  A kernel configured without `CONFIG_INITRAMFS_SOURCE` (a developer's own build) mounts the root file system as before.
- **The boot loader needs a prepared image.**  The phone's boot loader reads the address to load the kernel at, and the length to load, from two words in the header of the kernel image.  A mainline `zImage` does not have these values.  The `kernel-<model>` outputs do: they hold the `zImage` followed by the model's device tree, with the two header words filled in by `opendeskphone-image` (`crates/opendeskphone-image/src/kernel.rs`).
- **The boot loader adds its own kernel command line.**  The command line starts with the `bootargs` property of the device tree (`files/arch/arm/boot/dts/broadcom/bcmring-digium-common.dtsi`).  The boot loader then appends the flash partition table (`mtdparts=`), the root file system (`root=`), and a few more words, and those cannot be changed.  To add a kernel parameter, add it to `bootargs`.
- **Flash partitions must be found by name.**  The partition table comes from the boot loader, and partition offsets differ between phones, so user space looks partitions up by name in `/proc/mtd` and never by number.
- **The serial console is silent by default.**  The phone's serial port is also the port for an electronic hook switch headset adapter, so the kernel writes no messages to it unless it is told to.  To see kernel messages on the serial port, add `amba_pl011.console_output=1` to the kernel command line, or write 1 to `/sys/module/amba_pl011/parameters/console_output` on a running phone.
- **The kernel log survives a reset.**  The kernel keeps the last 8 KiB of its console output, and the last 8 KiB of the kernel log from a panic, in a small memory that keeps its contents across a reset.  After the next boot, mount the `pstore` file system on `/sys/fs/pstore` to read them.
- **The boot loader has no watchdog and no fallback.**  It starts the kernel of the selected firmware bank with the watchdog timer switched off.  If a kernel in flash stops before the firmware starts, nothing resets the phone, and every later power-on starts the same kernel again.  The phone then does not boot until its flash is rewritten through a JTAG adapter.  Try a new kernel from RAM before installing it, as described below.

## The D80's kernel

`nix build .#kernel-d80` builds the same source tree with `d80_defconfig`.  The result holds:

| Path | Contents |
| --- | --- |
| `zImage` | The compressed kernel. |
| `dtbs/imx6dl-digium-d80.dtb` | The device tree (`files/arch/arm/boot/dts/nxp/imx/imx6dl-digium-d80.dts`). |
| `zImage-dtb` | `zImage` followed by the device tree, which is the form the boot loader starts. |
| `lib/modules/7.3.0/` | The loadable modules (sound, Wi-Fi, Bluetooth, and some file systems), and `modules.dep`, from which the firmware takes the order to load them in. |
| `config` | The full configuration that was used. |

The port adds one driver, for the ZL38040 voice processor (`files/sound/soc/codecs/zl38040.c`), and an entry for the RTL8363NB Ethernet switch in the `rtl8365mb` driver.  Everything else is the device tree and the configuration.

`nix build .#boot-d80` wraps `zImage-dtb` as the boot image, with `opendeskphone-image d80 boot-image`.  The build fails if the image does not fit the 8 MiB partition.

The kernel has no early-boot stage.  The D80 has one kernel and one root file system, so there is no firmware bank to select: the kernel mounts the root file system itself, read-only, from the fifth partition of the eMMC (`root=/dev/mmcblk0p5 rootfstype=squashfs` in `CONFIG_CMDLINE`).  The boot image's ramdisk is an empty archive.  With no `/init` in it, the kernel goes on to mount the root file system.

The recovery image has the same kernel.  Its ramdisk has an `/init`, the recovery program (`crates/opendeskphone-recovery`), which the kernel runs in place of mounting the root file system.  The ramdisk is an XZ-compressed archive (`CONFIG_RD_XZ`).  The build fails if the image does not fit the 8 MiB recovery partition, so a larger kernel leaves less room for the ramdisk.

The firmware loads the sound and Bluetooth modules when it starts, from the lists in `crates/opendeskphone/src/init/modules.rs`.  It loads no Wi-Fi module: the firmware does not use Wi-Fi.

### Constraints to know about

- **The boot loader starts an Android boot image.**  The phone's U-Boot reads the image from the first partition of the eMMC, which is 8 MiB.  The image has a version 0 header and 2048-byte pages.  Its kernel is `zImage-dtb`, loaded at 0x10808000, and its ramdisk is loaded at 0x11800000.  The image always has a ramdisk: the stock U-Boot has not been seen to start an image without one.  U-Boot passes the memory layout and the location of the ramdisk in ATAGs, which the kernel merges into the appended device tree (`CONFIG_ARM_ATAG_DTB_COMPAT`).
- **The kernel ignores the boot loader's command line.**  U-Boot passes the command line of the stock Android system, which has `init=/init` and parameters for the stock kernel's drivers.  The configuration replaces it with its own (`CONFIG_CMDLINE` with `CONFIG_CMDLINE_FORCE`).  To add a kernel parameter, add it to `CONFIG_CMDLINE` in `configs/d80_defconfig`.
- **The root file system must be SquashFS.**  `CONFIG_CMDLINE` names the file system type, and `CONFIG_SQUASHFS` is built in for it.  To change the root file system's partition or type, change `CONFIG_CMDLINE`, the image build (`nix/image.nix`), and the partition that the firmware takes for the installed one (`D80_ROOT` in `crates/opendeskphone/src/init/nvdata.rs`).
- **Three devices need firmware files that are not in the repository.**  The image build takes them from the vendor's stock D80 image, which Nix downloads (`nix/d80-firmware.nix`).  The kernel loads them from `/lib/firmware`:

  | Device | File |
  | --- | --- |
  | ZL38040 voice processor | `zl38040.fw`: the stock firmware (`ZLS38040App_v2.2.2.s3`, built into the stock kernel), in the form that the kernel's `ihex2fw -j` gives it.  `opendeskphone-image d80 zl38040-firmware` makes it from the stock boot image.  The driver does not start without it. |
  | Wi-Fi (BCM43430 revision 0, in the AP6212 module) | `brcm/brcmfmac43430a0-sdio.bin` and the board's NVRAM file, `brcm/brcmfmac43430a0-sdio.txt` |
  | Bluetooth (in the same module) | `brcm/BCM4343A0.hcd` |

- **Sound is one PCM device, and a mixer control selects the path.**  The control `Audio Path` of the ZL38040 driver selects `Speaker`, `Handset`, `Headset`, or `Bluetooth`.  `Master Playback Volume` is 0 to 10 and starts at 0, which is silence.  The PCM device is stereo at 8 or 16 kHz, with the voice in the left channel in both directions.
- **The panel is mounted upside down.**  The first line of the frame buffer is at the bottom of the screen as the phone is viewed, and the device tree says so (`rotation = <180>`).  The kernel does not rotate the picture, and the touchscreen reports positions in the frame buffer's coordinates.
- **`CONFIG_DRM_IMX_PARALLEL_DISPLAY` is on only to make the kernel link.**  In Linux 7.3, the LVDS driver (`CONFIG_DRM_IMX_LDB`) needs `CONFIG_DRM_DISPLAY_HELPER` and does not select it.  The parallel display driver, which the phone does not use, does.
- **`make dtbs_check` reports two problems in the device tree.**  The panel has no compatible for its part, which is unknown.  The Ethernet switch has an `mdio` node, which the driver requires and the binding allows only for a switch on the other kind of bus.

## Working on the kernel

The ports are developed in an ordinary git checkout of the Linux `v7.3` tag, outside this repository.  The `patches/`, `files/`, and `configs/` directories are generated from that checkout.

To create the checkout with the ports applied:

```text
git clone --depth 1 --branch v7.3 https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git ~/linux-bcmring
cp -r --no-preserve=mode "$(nix build .#kernel-source --no-link --print-out-paths)"/. ~/linux-bcmring/
```

The checkout's `HEAD` stays at `v7.3`, so `git status` and `git diff` in it show exactly the ports.

After editing the checkout, regenerate this directory and build:

```text
cargo xtask kernel-sync ~/linux-bcmring
nix build .#kernel
```

`kernel-sync` replaces the contents of all three directories.  It writes every modified file of the checkout into a patch in `patches/`, with one numbered patch for each subsystem, copies every new file to `files/`, and copies the files under `arch/arm/configs/` to `configs/`.  It reads `git diff HEAD` and the list of untracked files, so the checkout's `HEAD` must be `v7.3`.

## Trying a kernel without installing it

On a phone that runs Open Desk Phone, this command starts the kernel, the device tree, and the root file system of a firmware image from RAM and writes nothing to flash:

```text
dphone ramboot IMAGE.eff
```

`IMAGE.eff` is the firmware image that `nix build .#eff-<model>` produces.  A reboot or a power cycle brings the installed firmware back.  If the new kernel stops, the watchdog timer resets the phone, and the installed firmware starts.  `dphone ramboot --help` lists the options, and `crates/opendeskphone-update/src/ramboot.rs` explains how the command works.

The command depends on the following parts of the kernel:

- The running kernel must be built with `CONFIG_KEXEC`, which lets one kernel start another.
- The new kernel must be built with `CONFIG_MTD_PHRAM`, which presents a range of RAM as a flash device.  The command places the image's root file system in RAM and passes its location in the `phram.phram=` kernel parameter.
- The watchdog timer keeps running while the new kernel starts.  The new kernel keeps it from expiring for 120 seconds (`CONFIG_WATCHDOG_OPEN_TIMEOUT`).  After that, the firmware must have opened `/dev/watchdog`, or the phone resets.
- The command passes the kernel parameter `bcmring.coldsim` to the new kernel.  With it, the kernel resets the peripherals and stops their clocks early in its start, so that the drivers find the peripherals as they are after a power-on, and not as the running system left them.
