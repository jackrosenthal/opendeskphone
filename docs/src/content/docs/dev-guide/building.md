---
title: "Building from source"
---

## Prerequisites

- **An x86-64 Linux machine.**  The Nix flake defines builds for `x86_64-linux` only.
- **[Nix](https://nixos.org/download/)**, with the `nix-command` and `flakes` features enabled.  Nix provides every other tool, including the Rust toolchain, the ARM C compiler for the kernel, and Node.js for the web interface.
- **A network connection for the first build.**  Nix downloads the Linux source and two stock firmware images from the vendor's server.  The voice DSP firmware is proprietary and not in the repository, so the image build extracts the DSP firmware from the stock images.

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

The D80 has no image yet.  Its `dphone` binary and its kernel are separate builds; see [Phone families](#phone-families) and the table below.

### Image components

Each component of an image is a separate flake output.

| Output | Result |
| --- | --- |
| `opendeskphone` | The firmware binary for the D40 thru D70: static, for ARMv6, without the D80's touch skin. |
| `opendeskphone-d80` | The firmware binary with the D80's touch skin.  No image contains it yet. |
| `opendeskphone-early` | The early-boot stage. |
| `kernel` | The kernel of the D40, D45, D50, D60, D62, D65, and D70 (`zImage`), a device tree for each of those models, and the loadable modules. |
| `kernel-<model>` | `kernel.bin`: the kernel and the model's device tree, as written to a kernel partition.  `<model>` is `d40`, `d45`, `d50`, `d60`, `d62`, `d65`, or `d70`. |
| `kernel-d80` | The D80's kernel (`zImage`), its device tree, the two as one file (`zImage-dtb`), and the loadable modules.  No image contains it yet.  See `kernel/README.md`. |
| `kernel-source` | The Linux tree with the ports applied and nothing built, for editing. |
| `apps` | The built-in apps, compiled for the phone. |
| `web` | The web frontend bundle that `dphone` embeds. |
| `brush`, `coreutils`, `sed` | The shell and utilities of the root file system. |
| `rootfs-tree`, `rootfs` | The root file system as a directory, and as the JFFS2 image in the EFF image. |
| `opendeskphone-image` | The host tool that packs and unpacks EFF images, JFFS2 images, and kernel images. |

Any change under `kernel/` rebuilds both kernels: a Nix build cannot reuse object files from an earlier build.

To show the model and version of an EFF image:

```sh
nix run .#opendeskphone-image -- eff info result
```

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

The D40 thru D70 have little flash, and the D80's touch skin has the largest fonts and artwork of any skin, so the binary in their images is built without `d80`.  That binary has none of the touch skin's fonts, icons, and artwork, and none of the code that only a touchscreen uses, e.g., the on-screen keyboard.  It does not start as a D80: `dphone main --model d80` logs an error and exits.

The feature `d80` enables the feature `touch` of `opendeskphone-ui`.  With `touch` off, the build script of `opendeskphone-ui` generates no touch skin tables, and `Theme::touch()` is false at compile time, so the compiler removes the code behind it.  Code that only the touch skin needs must be behind `Theme::touch()` to stay out of the small build.

A build with default features has every model.  The simulator, `cargo test`, and Clippy use the default features, so one test run covers all eight models.  A crate that depends on `opendeskphone-ui` outside its tests must set `default-features = false`: otherwise cargo enables `touch` in the binary of the D40 thru D70.

The Nix build guards against that.  The touch skin's icon data begins with a marker string (`TOUCH_MARK` in `crates/opendeskphone-ui/build.rs`).  The build of `.#opendeskphone` fails if the binary contains the marker, and the build of `.#opendeskphone-d80` fails if the binary lacks it.

`.#opendeskphone-d80` is built for the same ARMv6 target as the other binary, as a stand-in until the D80's hardware support exists.

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

CI (`.github/workflows/ci.yml`) also uploads the image of every model as a workflow artifact named `firmware-<model>`, so a pushed branch's images need no local build.

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
