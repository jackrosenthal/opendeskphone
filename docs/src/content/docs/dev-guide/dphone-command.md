---
title: "dphone command reference"
---

The firmware's program, `dphone`, is also the phone's command-line tool.  Run the commands in a shell on the phone: over [SSH or the serial console](ssh-and-serial-console.md), or on the **Shell** page of the web interface.  `dphone --help` lists the commands; `dphone <command> --help` shows one command's arguments and options.

## Local and remote commands

- **Local commands** run in the calling process and work while the main role is down or in safe mode: the firmware update commands, `dphone ramboot`, `dphone completion`, and the system commands.
- **Remote commands** go over a local socket to the main role, which does the work and returns the output.  Without a running main role, a remote command fails with a message that the main role cannot be reached.

## Firmware

Local.  For usage, see [Firmware updates](firmware-updates.mdx) and [Test builds](test-builds.md).

| Command | Description |
| --- | --- |
| `dphone update status` | Shows both firmware banks, the `ctrl` record, and the bank that starts next. |
| `dphone update install <file, -, or URL>` | Installs an image into the inactive bank and selects that bank. |
| `dphone update switch-bank <a or b>` | Selects a bank for the next boot without writing an image. |
| `dphone update arm-trial` | Puts the bank selected for the next boot on trial. |
| `dphone update abandon` | Cancels the trial of an installed image. |
| `dphone update write-userdata <image>` | Erases the unused `userdata` partition and writes a root file system image there, to boot a test kernel with a root file system outside both banks. |
| `dphone ramboot <image>` | Boots an image from RAM without installing anything. |

## System

Local.  The root file system has no separate programs for these commands: each name is a link to `dphone`, so `dmesg` and `dphone dmesg` are the same command.

| Command | Description |
| --- | --- |
| `reboot`, `poweroff`, `halt` | Restart the phone, power the phone off, or halt the system. |
| `dmesg` | Prints the kernel log. |
| `ps` | Lists all processes. |
| `mount`, `umount` | Mount a file system or list the mounts, and unmount a file system. |
| `lsmod`, `modprobe`, `insmod`, `rmmod` | List, load, and unload kernel modules. |

## Main role

| Command | Description |
| --- | --- |
| `dphone main` | Runs the main role.  On a phone, the init role starts the main role; run the command directly only for the [simulator](simulator.md).  `--safe` starts only SSH, the network, and the diagnostics; `--verbose` logs at the debug level. |
| `dphone supervise` | Runs the init role's supervisor outside process 1, for the tests. |

## Shell

Local.

| Command | Description |
| --- | --- |
| `dphone completion` | Prints the Bash completion of `dphone`'s commands and options.  The phone's shell loads it the first time <kbd>Tab</kbd> completes a `dphone` command line. |

## Status and logs

Remote.  For usage, see [Logs and diagnostics](diagnostics.md).

| Command | Description |
| --- | --- |
| `dphone ping` | Checks that the main role responds. |
| `dphone version` | Shows the running version and the model. |
| `dphone health` | Shows the conditions behind the health report that confirms a trial boot.  Exits with 0 when the main role reports healthy, 1 otherwise. |
| `dphone log` | Shows the latest log entries.  `-n <count>` sets the number of entries; `-f` follows the log. |
| `dphone crashlog` | Prints the records of the last kernel panic, saved by the main role in `/nvdata/opendeskphone/crash/`. |
| `dphone state [topic]` | Dumps the state store, which every subsystem publishes to: calls, accounts, network, audio, firmware, and more.  A topic limits the dump to the fields whose names contain the word, e.g., `dphone state audio`. |
| `dphone restart` | Restarts the main role only.  The kernel and the init role keep running. |

## Configuration

Remote.

| Command | Description |
| --- | --- |
| `dphone config list [text]` | Lists the settings of the merged configuration as `ID=VALUE`, limited to the IDs that contain the text.  Secrets are masked. |
| `dphone config get <id>` | Prints one setting. |
| `dphone config set <id> <value>` | Changes and applies a setting. |
| `dphone config reload` | Merges the configuration files again and applies the result, e.g., after an edit in nvdata. |

## Phone control

Remote.

| Command | Description |
| --- | --- |
| `dphone key <key>...` | Presses keys in sequence, e.g., `dphone key 1 2 3 sk1`.  `dphone key --help` lists the key names. |
| `dphone touch <x> <y>` | Taps the D80's touchscreen at a pixel of the screen, counted from the top-left corner, e.g., `dphone touch 400 640` for the middle of the 800x1280 screen.  `--to <x> <y>` drags the finger from the first place to the second instead, e.g., `dphone touch 400 1000 --to 400 300` to scroll a list; `--steps` sets the number of moves in the drag (default 10).  A model without a touchscreen refuses the command. |
| `dphone hook off`, `dphone hook on` | Lift and replace the handset. |
| `dphone call` | Lists the calls.  `dial <number>` calls a number, `answer` answers the first call, and `hangup` ends every call. |
| `dphone app` | Lists the installed apps.  `launch` starts an app or brings a running app to the front; `close` closes an app. |
| `dphone screenshot <file>` | Saves a display's contents: PPM for a color display, PGM for a grayscale display.  `--screen side` selects the D70's second display.  A relative file name resolves against the main role's working directory, so use an absolute path, e.g., `/tmp/screen.ppm`. |
| `dphone alarms` | Shows the alarm clock's alarms.  `add`, `remove`, `snooze`, and `dismiss` act on alarms. |
| `dphone page` | Shows the state of multicast paging.  `start <group>` pages a group; `stop` ends a page. |

To view a phone's screen on the workstation:

```sh
ssh root@192.0.2.20 dphone screenshot /tmp/screen.ppm
scp root@192.0.2.20:/tmp/screen.ppm .
```

## Hardware and subsystems

Remote.  Safe mode starts only the network, so in safe mode `dphone net` works and the other commands report the subsystem unavailable.

| Command | Description |
| --- | --- |
| `dphone net` | Shows the interfaces, the addresses, and the network task's current activity.  `bridge on` and `bridge off` toggle the bridge to the PC port.  `plain` applies the simplest configuration (DHCP, no VLAN, PC port down); `configured` reapplies the configured settings.  `sync-time` synchronizes the clock with the time server. |
| `dphone audio` | Shows the audio engine, the codecs, the levels, and the audio state.  `path`, `tone`, `ring`, `volume`, and `mute` control the audio manually. |
| `dphone dsp` | Shows the voice DSP's state.  `selftest` runs the checks that need no hardware; `restart` reloads the DSP firmware.  `job`, `bands`, and `canceller` are experiments for work on the DSP interface. |
| `dphone hal` | Shows the model and the devices found and missing.  `lamp`, `backlight`, `contrast`, and `pattern` set the lamps and the displays, but only before the user interface has started, e.g., in a main role started without the user interface. |
| `dphone ehs` | Shows the state of the EHS headset adapter, and whether the serial console has the port. |
| `dphone expansion` | Shows the expansion modules. |

## New commands

A remote command is a `clap` type that a subsystem defines and registers a handler for at startup.  To add the command to the command tree and to `dphone --help`, list the type in the `Remote` enumeration in `crates/opendeskphone/src/cli.rs`, whose top comments describe the design.
