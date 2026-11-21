---
title: "Simulator"
---

Run the commands on this page from the root of the repository, in a [`nix develop` shell](building.md#cargo-workspace).

## Phone simulator

`dphone main --sim` runs the main role on the hardware layer's simulator backend: the user interface draws to an in-memory frame buffer, the audio devices are simulated, and the workstation's network configuration is untouched.

```sh
cargo run -p opendeskphone -- main --sim --model d60 --nvdata /tmp/opendeskphone-sim
```

`--nvdata` names the directory that serves as nvdata.  `--model` accepts any of the seven models, and `d80`, whose touchscreen user interface is under development (the simulator is the only place it runs); the default is `d60`.  The main role logs to the terminal.

The D80 has no keys: operate it by touch, either on the web interface's **Screen** page, where a click on the mirrored screen is a tap and a drag is a finger's drag, or with `dphone touch` (see below).  The Screen page draws the D80 schematically, since no references for its look have been gathered yet.

| Interface | Details |
| --- | --- |
| Web interface | `http://127.0.0.1:8080/`, or the port given by `--web-port`.  A new nvdata directory has the default login: user `admin`, password `789`.  The **Screen** page mirrors the simulated display and takes key presses, or on the D80, touches. |
| SIP | Binds port 5060 on the workstation.  As with an empty nvdata on a phone, the simulator starts at the menu that asks for a server.  Provision the simulator from a DPMA server, or add SIP accounts in the web interface. |
| SSH | Only with `--ssh-listen <address>`, e.g., `--ssh-listen 127.0.0.1:2222`, and only while the [SSH setting](ssh-and-serial-console.md#ssh) is on. |
| Apps | The built-in apps are missing unless `--apps-root` (or the environment variable `DPHONE_APPS_ROOT`) names a directory of staged apps.  For staging, see `crates/opendeskphone-apps/README.md`.  `--ringtones` and `--alarm-tones` (`DPHONE_RINGTONES`, `DPHONE_ALARM_TONES`) name the tones, e.g., `assets/ringtones` and `assets/alarm-tones`. |

### `dphone` commands

Commands that the main role answers reach the simulator through a socket in the run directory: `.run` in the nvdata directory, or the directory given by `--run-dir`.  Give the socket's path with `dphone --diag-socket`, or set the environment variable `DPHONE_DIAG_SOCKET` to it:

```sh
export DPHONE_DIAG_SOCKET=/tmp/opendeskphone-sim/.run/diag.sock
cargo run -q -p opendeskphone -- ping
cargo run -q -p opendeskphone -- key 1 2 3
cargo run -q -p opendeskphone -- touch 400 640   # the D80
cargo run -q -p opendeskphone -- screenshot /tmp/screen.ppm
cargo run -q -p opendeskphone -- log -f
```

Commands that touch flash or the kernel, such as `dphone update` and `dphone ramboot`, require a phone.  For all commands, see the [`dphone` command reference](dphone-command.md).

:::note[Keep the nvdata path short]
Unix socket paths are limited to about 100 characters.  With a longer path, the main role logs that the socket path is too long, and commands cannot reach the simulator.  Use a shorter nvdata directory or a short `--run-dir`.
:::

## Web interface simulator

An example program in `crates/opendeskphone-web` runs the web server on a state store filled with mock calls, accounts, contacts, and apps.  Commands behave plausibly: a dialed call rings and connects, and a firmware installation steps through the phases.

```sh
cargo run -p opendeskphone-web --example sim -- --model d60 --port 8480
```

The interface is at `http://127.0.0.1:8480/`; the login is `admin`, password `789`.  The example's display shows the keys that are pressed, not the phone's user interface; with `--model d80`, it marks where the screen is touched with a cross.

For live reload, run the Vite development server, which proxies API requests to the example program on port 8480:

```sh
cd web
pnpm install --frozen-lockfile
pnpm dev
```

To use a real phone instead, set the environment variable `PHONE_URL` to the phone's URL, e.g., `PHONE_URL=http://192.0.2.20 pnpm dev`.

After changing the service definitions in `proto/`, run `pnpm gen` in `web/` to regenerate the committed `web/src/gen`.
