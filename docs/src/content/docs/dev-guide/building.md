---
title: "Building from source"
---

## Prerequisites

- **An x86-64 Linux machine.**  The Nix flake defines builds for `x86_64-linux` only.
- **[Nix](https://nixos.org/download/)**, with the `nix-command` and `flakes` features enabled.  Nix provides every other tool, including the Rust toolchain, the ARM C compiler for the kernel, and Node.js for the web interface.
- **A network connection for the first build.**  Nix downloads the Linux source and two stock firmware images from the vendor's server.  The voice DSP firmware is proprietary and not in the repository, so the image build extracts the DSP firmware from the stock images.  A build for the D80 downloads the stock D80 image (195 MB) for the same reason: see [The D80's image](#the-d80s-image).

```sh
git clone https://github.com/jackrosenthal/opendeskphone.git
cd opendeskphone
```

## Firmware images

Each model has a flake output `eff-<model>`, where the model is `d40`, `d45`, `d50`, `d60`, `d62`, `d65`, or `d70`.  From the root of the repository:

```sh
nix build .#eff-d60
```

The `result` link points to the image in the Nix store, named `<version>_<model>_firmware.eff`, e.g., `3_0_0_D60_firmware.eff`.  The version is the workspace version in the root `Cargo.toml`.  `nix build .#all` builds the images of all seven models into one directory.

:::note[Nix builds only what git tracks]
A flake in a git repository sees only the files that git tracks, including uncommitted changes to those files.  A new file is invisible to the build until `git add`.
:::

The kernel, the root file system, and the `dphone` binary are the same for these seven models.  The images differ only in the device tree attached to the kernel and in the model name in the image header.

The D80's image is built differently and has not run on a phone: see [The D80's image](#the-d80s-image).

### Image components

Each component of an image is a separate flake output.

| Output | Result |
| --- | --- |
| `opendeskphone` | The firmware binary for the D40 thru D70: static, for ARMv6, without the D80's touch skin. |
| `opendeskphone-d80` | The D80's firmware binary: static, for ARMv7 with hardware floating point, with the touch skin. |
| `opendeskphone-early` | The early-boot stage. |
| `kernel` | The kernel of the D40, D45, D50, D60, D62, D65, and D70 (`zImage`), a device tree for each of those models, and the loadable modules. |
| `kernel-<model>` | `kernel.bin`: the kernel and the model's device tree, as written to a kernel partition.  `<model>` is `d40`, `d45`, `d50`, `d60`, `d62`, `d65`, or `d70`. |
| `kernel-d80` | The D80's kernel (`zImage`), its device tree, the two as one file (`zImage-dtb`), and the loadable modules.  See `kernel/README.md`. |
| `boot-d80` | The D80's Android boot image: `zImage-dtb` and an empty ramdisk. |
| `rootfs-tree-d80`, `rootfs-d80` | The D80's root file system as a directory, and as the SquashFS image in its OTA package. |
| `kernel-source` | The Linux tree with the ports applied and nothing built, for editing. |
| `apps` | The built-in apps, compiled for the phone. |
| `web` | The web frontend bundle that `dphone` embeds. |
| `brush`, `coreutils`, `sed` | The shell and utilities of the root file system.  `brush-d80`, `coreutils-d80`, and `sed-d80` are the D80's. |
| `rootfs-tree`, `rootfs` | The root file system as a directory, and as the JFFS2 image in the EFF image. |
| `opendeskphone-image` | The host tool that packs and unpacks EFF images, JFFS2 images, and kernel images, and makes the D80's boot image and OTA package. |

Any change under `kernel/` rebuilds both kernels: a Nix build cannot reuse object files from an earlier build.

To show the model and version of an EFF image:

```sh
nix run .#opendeskphone-image -- eff info result
```

## The D80's image

The D80 is built on Android hardware: its storage is an eMMC with the stock Android partitions, its boot loader starts an Android boot image, and its stock recovery installs an Android OTA package, which is a zip file.  Two outputs hold the image:

```sh
nix build .#ota-d80
nix build .#eff-d80
```

| Output | Result |
| --- | --- |
| `ota-d80` | `<version>_D80_firmware.zip`: the OTA package, signed with the key in the repository. |
| `eff-d80` | `<version>_D80_firmware.eff`: the same package in the vendor's EFF envelope. |

:::caution[Not yet run on a phone]
No D80 has started this image.  The kernel in it has run on a D80, from a test ramdisk.  The `dphone` binary's D80 hardware support (the display, the touchscreen, the hookswitch, the lamp, and the audio) is written from the kernel's interfaces and has run only in the tests on the workstation.
:::

The package writes two partitions and leaves the rest of the eMMC as stock made it:

| Partition | Size | Stock use | Open Desk Phone |
| --- | --- | --- | --- |
| `mmcblk0p1` | 8 MiB | Boot image | The boot image (`boot-d80`). |
| `mmcblk0p2` | 8 MiB | Recovery | Not written.  The stock recovery stays. |
| `mmcblk0p5` | 512 MiB | `/system` | The root file system (`rootfs-d80`), mounted read-only. |
| `mmcblk0p6` | 512 MiB | `/cache` | nvdata, on the ext4 file system that stock made.  Not written by the package. |
| `mmcblk0p4` | The rest | `/data` | Not used.  The stock firmware's settings stay. |

The package contains `boot.img`, `rootfs.img`, the script that writes them (`META-INF/com/google/android/updater-script`), and the program that runs the script (`update-binary`).  The program is the stock package's own, because the stock recovery runs whatever program a package brings.

The package is signed the way Android's `signapk -w` signs a whole file: the signature (RSA with SHA-1, as in the stock package) and the certificate are in the zip file's comment.  The key and its certificate are `d80-ota.key.pem` and `d80-ota.x509.pem` in `crates/opendeskphone-image/keys`.  The private key is in the repository, so the signature does not show who built a package: anyone can make it.  It is there for a recovery that installs only signed packages and has this certificate among its keys.

The image build takes four more files from the stock D80 image, which Nix downloads from the vendor (`nix/d80-firmware.nix`): the firmware of the voice processor, the firmware and the settings file of the Wi-Fi module, and the patch file of the Bluetooth module.

Three things about installing the package are not settled:

- **The signature.**  A stock package is signed with the vendor's key, and the stock firmware's updater also checks a signature at the end of an EFF image.  The package built here is signed with the key in the repository instead, and the EFF image has no signature at its end.  The stock updater passes a bare zip file to the recovery without the EFF check.  Whether the stock recovery installs a package that is not signed with the vendor's key is not known.
- **The way back.**  The package overwrites the stock `/system` partition.  The stock recovery in `mmcblk0p2` can install a stock package again, but `dphone` cannot start the recovery yet.
- **Updates from the running phone.**  `dphone update install` refuses on a D80.  A D80 has no firmware banks, so its installer would be the recovery.

The D80's `dphone` differs from the other models' in these ways:

- It has no voice DSP.  The echo canceller is in the phone's voice processor, and the rest of the DSP's work runs in `dphone` (`crates/opendeskphone-media/src/softdsp.rs`).  Calls use G.711 or G.722.  G.726 is not available.
- The handset, headset, and speaker share one sound device, and a mixer control selects the path.
- The picture is drawn upside down in the frame buffer, because the panel is mounted upside down, and touch positions are turned the same way.
- A Bluetooth headset's audio does not pass through `dphone`.  The voice processor exchanges it with the Bluetooth module directly, and `dphone` selects the voice processor's Bluetooth path.  With a headset that negotiates codecs, the link carries wide-band speech (mSBC, 16 kHz).

## Cargo workspace

`nix develop` opens a shell with the Nix build's Rust toolchain, the targets for the phone and for WebAssembly, and the other tools that the build scripts and the web frontend need.

Three Open DPMA crates (`dpma-sys`, `dpma-ast`, and `res_open_dpma`) read the headers of an Asterisk source tree and build only with `ASTERISK_SOURCE_DIR` set, so exclude them from workspace-wide commands:

```sh
cargo build --workspace --exclude dpma-sys --exclude dpma-ast --exclude res_open_dpma
```

`cargo build -p opendeskphone` builds the firmware binary for the workstation, which the [simulator](simulator.md) runs.  For the D40 thru D70, `.cargo/config.toml` sets the linker and the processor flags of the `arm-unknown-linux-musleabi` target:

```sh
cargo build --release -p opendeskphone --target arm-unknown-linux-musleabi --no-default-features --features web
```

### Phone families

The `dphone` binary is built in two ways, selected by the cargo feature `d80` of the `opendeskphone` crate:

| Build | Cargo arguments | Models |
| --- | --- | --- |
| Without the touch skin | `--no-default-features --features web` | D40, D45, D50, D60, D62, D65, and D70 |
| Every model | none: `d80` is a default feature | The same, and the D80 |

The D80's image has the second build, compiled for the D80's processor:

```sh
cargo build --release -p opendeskphone --target armv7-unknown-linux-musleabihf
```

The D40 thru D70 have little flash, and the D80's touch skin has the largest fonts and artwork of any skin, so the binary in their images is built without `d80`.  That binary has none of the touch skin's fonts, icons, and artwork, and none of the code that only a touchscreen uses, e.g., the on-screen keyboard.  It does not start as a D80: `dphone main --model d80` logs an error and exits.

The feature `d80` enables the feature `touch` of `opendeskphone-ui`.  With `touch` off, the build script of `opendeskphone-ui` generates no touch skin tables, and `Theme::touch()` is false at compile time, so the compiler removes the code behind it.  Code that only the touch skin needs must be behind `Theme::touch()` to stay out of the small build.

A build with default features has every model.  The simulator, `cargo test`, and Clippy use the default features, so one test run covers all eight models.  A crate that depends on `opendeskphone-ui` outside its tests must set `default-features = false`: otherwise cargo enables `touch` in the binary of the D40 thru D70.

The Nix build guards against that.  The touch skin's icon data begins with a marker string (`TOUCH_MARK` in `crates/opendeskphone-ui/build.rs`).  The build of `.#opendeskphone` fails if the binary contains the marker, and the build of `.#opendeskphone-d80` fails if the binary lacks it.

`.#opendeskphone-d80` is built for `armv7-unknown-linux-musleabihf`, and so are the shell and the utilities in the D80's image (`brush-d80`, `coreutils-d80`, and `sed-d80`).  The image build fails if a program in an image is built for the other family's processor.

### Web frontend

`dphone` embeds the web frontend from `web/dist`, or a placeholder page if `web/dist` does not exist.  To build the frontend:

```sh
cd web
pnpm install --frozen-lockfile
pnpm build
```

## Tests and checks

To run the tests of the firmware's crates on the workstation:

```sh
cargo test --workspace --exclude dpma-sys --exclude dpma-ast --exclude res_open_dpma
```

Tests that need the stock firmware images, an Asterisk server with Open DPMA, or the staged apps find them in `test-inputs/` at the top of the checkout, which is not in git.  To make it, a link to the Nix output of the same name:

```sh
nix build .#test-inputs -o test-inputs
```

A test whose input is missing, or that needs something the workstation lacks (the OpenSSH client, `qemu-arm`), prints `SKIPPED` and passes, so a passing `cargo test` does not cover everything.  The Nix check builds the tests with the feature `test-inputs/required`, which makes such a test fail instead.  To run the tests as the check does:

```sh
cargo test --workspace --exclude dpma-sys --exclude dpma-ast --exclude res_open_dpma --features test-inputs/required
```

Rebuild the link after a change to `open-dpma/` or to the apps: it holds Nix's build of them.

The built-in apps are a separate workspace.  To run the apps' unit tests on the workstation, with the alias from `apps/.cargo/config.toml`:

```sh
cd apps
cargo host-test
```

The flake's checks skip nothing except the [network namespace tests](#network-namespace-tests), and CI runs them on every push:

| Command | Description |
| --- | --- |
| `nix build .#checks.x86_64-linux.tests` | Runs the tests of every crate and of the apps, with the stock images, an Asterisk server with Open DPMA, and an SSH client, and requires the tests that need them to run. |
| `nix build .#checks.x86_64-linux.clippy` | Lints the workspace and the apps with Clippy.  Any warning fails the check. |
| `nix build .#checks.x86_64-linux.jffs2-mount` | In a virtual machine, mounts the root file system image with the kernel's JFFS2 driver, on simulated NAND flash and on a RAM device, and compares the files with the source directory.  Requires `/dev/kvm`. |
| `nix build .#checks.x86_64-linux.opendeskphone` | Builds the `dphone` binary for the D40 thru D70, and fails if the binary contains the D80's touch skin. |
| `nix build .#checks.x86_64-linux.opendeskphone-d80` | Builds the `dphone` binary with the D80's touch skin, and fails if the binary lacks the skin. |
| `nix flake check` | Runs every check, and builds both kernels, the utilities, and both `dphone` binaries. |

CI (`.github/workflows/ci.yml`) also uploads the image of every model as a workflow artifact named `firmware-<model>`, so a pushed branch's images need no local build.  The D80's EFF image and OTA package are the artifact `d80-image`.  A release does not include them.

The Open DPMA tests run in containers against each supported Asterisk version: `cargo xtask ci` in `open-dpma/`; see `open-dpma/README.md`.

### Network namespace tests

CI cannot run the tests that need unprivileged network namespaces.  Run them before a release, and after changing the network, provisioning, paging, or expansion code:

```sh
cargo test -p opendeskphone-net --features netns-tests,test-inputs/required --test netns
cargo test -p opendeskphone --features netns-tests,test-inputs/required --test pcport --test staticaddr --test paging --test expansion
```

## xtask commands

| Command | Description |
| --- | --- |
| `cargo xtask deploy <phone>` | Builds `dphone` for the phone, pushes the binary, and restarts the main role.  See [Binary deploy](test-builds.md#binary-deploy). |
| `cargo xtask kernel-sync <linux tree>` | Regenerates `kernel/patches`, `kernel/files`, and `kernel/configs` from a Linux checkout that contains the port.  For the procedure, see `kernel/README.md`. |
| `cargo xtask tz-table` | Regenerates the phone's table of time zone rules from the workstation's zone database. |
