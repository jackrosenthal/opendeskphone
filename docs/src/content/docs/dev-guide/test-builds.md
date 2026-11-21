---
title: "Test builds"
---

Two ways run new code on a phone without writing to flash.  Both require a phone running Open Desk Phone with [SSH](ssh-and-serial-console.md#ssh) on.

| | Binary deploy | RAM boot |
| --- | --- | --- |
| Command | `cargo xtask deploy` on the workstation | `dphone ramboot` on the phone |
| Replaces | The main role | The kernel, the device tree, and the whole root file system |
| Keeps | The kernel, the init role, and the root file system | nvdata |
| Undo | Reboot | Reboot, power cycle, or wait for the time limit |

## Binary deploy

For a change to the main role, i.e., anything in the `dphone` binary but the init role, one command builds the binary for the phone, copies the binary over SSH, and restarts the main role:

```sh
cargo xtask deploy 192.0.2.20
```

The argument is the phone's address or host name, with the login `root` unless written as `LOGIN@HOST`.  `--no-build` pushes the binary of the previous build.

The command builds the binary as the images of the D40 thru D70 have it, without the D80's touch skin.  See [Phone families](building.md#phone-families).

The command copies the binary to `/tmp/opendeskphone`.  While that file exists, the init role runs `/tmp/opendeskphone` as the main role instead of the installed binary.  `/tmp` is in RAM, so a reboot restores the installed binary.  To restore the installed binary without a reboot:

```sh
ssh root@192.0.2.20 'rm /tmp/opendeskphone && dphone restart'
```

If a pushed binary fails three times in a row, each time within 30 seconds of starting, the init role runs the installed binary until the next boot.

### Limitations

A pushed binary does not update:

- **The init role.**  Process 1 remains the installed binary.  A change to the init role requires an image.
- **The built-in apps.**  The apps are compiled files in the root file system, tied to the binary's app runtime.  If a change alters the runtime, the pushed binary refuses to load the installed apps.
- **The web frontend, unless built.**  The binary embeds `web/dist`.  Run `pnpm build` in `web/` before deploying, or the pushed binary serves a placeholder page.  See [Web frontend](building.md#web-frontend).

## RAM boot

`dphone ramboot` boots an image's kernel, device tree, and root file system from RAM, without writing to the firmware banks or the `ctrl` partition: to test a kernel before flashing, or a whole image without a flash write.

The command accepts only an Open Desk Phone EFF image for the phone's model, as built by `nix build` and installed by `dphone update install`.  Loading briefly takes free memory for two copies of the image, so a copy in `/tmp` would exhaust the memory.  Pipe the image over SSH instead:

```sh
ssh root@192.0.2.20 dphone ramboot - < result
```

`result` is the link that `nix build .#eff-d60` creates.  The command verifies the image, prints the memory layout and the kernel command line, and hands control to the new kernel.  The SSH session ends, and the phone is reachable again in about a minute.

In the RAM-booted system:

- nvdata is the phone's nvdata.  The settings, the network address, and the SSH keys are unchanged, so the same `ssh` command works.  Writes to nvdata persist after the phone returns to the installed firmware.
- `dphone update status` reports that the phone is running from neither bank.
- A pending trial boot is neither counted nor confirmed.
- The full `dphone update` command is available, e.g., to [install the image](firmware-updates.mdx#dphone-update-install) once the image works.

### Return to the installed firmware

The phone returns to the installed firmware when:

- you run `dphone reboot` or power-cycle the phone,
- the kernel hangs: the hardware watchdog keeps running across the handover and resets the phone, or
- the time limit expires: 900 seconds, or the value of `--max-uptime`.

The time limit returns an unreachable system, e.g., one without a working network, without intervention.  To stay past the limit, create `/run/opendeskphone/stay` in the RAM-booted system:

```sh
ssh root@192.0.2.20 touch /run/opendeskphone/stay
```

### Options

| Option | Description |
| --- | --- |
| `--dry-run` | Verifies the image, prints what would be loaded and passed to the kernel, and stops. |
| `--max-uptime <seconds>` | The time limit.  The default is 900; 0 disables the limit. |
| `--append <arguments>` | Adds kernel parameters, e.g., `amba_pl011.console_output=1` for [early kernel messages](ssh-and-serial-console.md#early-kernel-messages) on the serial console. |

To boot an image with no time limit:

```sh
ssh root@192.0.2.20 dphone ramboot --max-uptime 0 - < result
```

### Kernel requirements

A kernel configuration change can break later runs of `dphone ramboot`, which requires:

- `CONFIG_KEXEC` in the running kernel.
- `CONFIG_MTD_PHRAM` in the new kernel, which sees the root file system in RAM as a flash device.
- A new kernel that keeps the watchdog from expiring until the init role opens `/dev/watchdog`.  `CONFIG_WATCHDOG_OPEN_TIMEOUT` allows 120 seconds; a system that takes longer to start is reset.
- The parameter `bcmring.coldsim`, passed to the new kernel, which makes the kernel reset the peripherals early so that drivers find the hardware in the power-on state.

For details, see `kernel/README.md`, and for the image's memory layout, `crates/opendeskphone-update/src/ramboot.rs`.
