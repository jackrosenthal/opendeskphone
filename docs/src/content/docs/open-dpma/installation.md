---
title: "Installation"
---

## Requirements

| Requirement | Details |
| --- | --- |
| Asterisk | Version 22, 23, or 24. |
| Platform | Linux on x86_64 or aarch64, with glibc 2.34 or later (e.g., Debian 12, Ubuntu 22.04, or RHEL 9).  For other platforms, [build from source](#building-from-source). |
| Asterisk modules | `res_pjsip`, `res_pjsip_session`, and `res_pjsip_pubsub`.  Without all three, Asterisk does not load Open DPMA. |
| `pjsip.conf` | The endpoint named by `default_outbound_endpoint` in the `[global]` section. |

Optional modules, used when loaded: `func_presencestate`, `app_voicemail`, `res_parking`, and `app_queue`, for the phones' Status, Voicemail, Parking, and Queues apps.

:::caution[A missing default outbound endpoint prevents Asterisk from starting]
The original module has the same requirement.  Define the endpoint before installing the module:

```ini title="pjsip.conf"
[global]
type=global
default_outbound_endpoint=dpma_endpoint

[dpma_endpoint]
type=endpoint
context=default
disallow=all
allow=ulaw
```
:::

## Download

Each [release](https://github.com/jackrosenthal/opendeskphone/releases) has a module named `res_open_dpma-asterisk<version>-<architecture>.so` for each supported Asterisk major version and architecture (`x86_64` or `aarch64`).  `asterisk -V` shows the Asterisk version, and `uname -m` the architecture.

## Setup

1. Copy the module into the Asterisk module directory as `res_open_dpma.so`:

    ```sh
    install -m 0755 res_open_dpma-asterisk22-x86_64.so /usr/lib/asterisk/modules/res_open_dpma.so
    ```

2. On a server that ran the original module, [disable the original module's three files](#migration-from-the-original-module).
3. Make sure that `res_digium_phone.conf` exists.  On a new server, start with the [minimal example](configuration.md#minimal-example).
4. Restart Asterisk.

## Migration from the original module

Open DPMA replaces the original module and the two PJSIP helper modules that ship with the original module.  None of the three may be loaded with Open DPMA.  Add these lines to `modules.conf`, or delete the three files from the module directory:

```ini title="modules.conf"
noload => res_digium_phone.so
noload => res_pjsip_endpoint_identifier_dpma.so
noload => res_pjsip_dialog_info_digium_body_supplement.so
```

The configuration file works unchanged.  Two other changes may be needed:

- Lines that use `chan_sip` must move to PJSIP.
- Scripts that reload the module must use the name `res_open_dpma.so`.

For all other changes, see [Differences from the original module](index.md#differences-from-the-original-module).

To switch back, delete `res_open_dpma.so` from the module directory, remove the three `noload` lines, and restart Asterisk.

## Verification

In the Asterisk CLI:

1. Check that the module is loaded and reports a version:

    ```text
    *CLI> module show like res_open_dpma
    *CLI> digium_phones show version
    Open DPMA version 3.1.1
    ```

2. Check the `[general]` settings, especially the two authentication lines:

    ```text
    *CLI> digium_phones show settings
    ```

3. Check that every phone section in the configuration file is listed:

    ```text
    *CLI> digium_phones show phones
    ```

4. Start a phone, then check that the phone has a session with `Auth:Yes` and `Configured: Yes`, and a configuration token:

    ```text
    *CLI> digium_phones show sessions
    *CLI> digium_phones show tokens
    ```

5. Check that the phone's lines are registered:

    ```text
    *CLI> pjsip show contacts
    ```

## Logging

The module logs most messages, e.g., why a phone was refused a configuration (`Auth failed for ip ...`), at a custom log level, `DPMA`.  Asterisk writes these messages only to log channels that list `dpma`:

```ini title="logger.conf"
[logfiles]
console => notice,warning,error,dpma
messages.log => notice,warning,error,dpma
```

## Building from source

Requirements: Rust 1.99 or later, libclang, and the configured headers of the target Asterisk version, i.e., an Asterisk source tree after `./configure`, named by the environment variable `ASTERISK_SOURCE_DIR`.

```sh
git clone https://github.com/jackrosenthal/opendeskphone.git
cd opendeskphone/open-dpma
export ASTERISK_SOURCE_DIR=/usr/src/asterisk-22.11.0
cargo build --profile module -p res_open_dpma
```

The build writes `target/module/libres_open_dpma.so` at the root of the repository.  Install that file as `res_open_dpma.so`, as in [Setup](#setup).

With podman or docker, `cargo xtask headers` instead copies the configured headers from the container image of the Asterisk release that the project tests against, and prints the directory:

```sh
export ASTERISK_SOURCE_DIR=$(cargo xtask headers --asterisk 22)
```

:::caution[Do not enable the `devtest` feature]
The crate's `devtest` feature adds the test suites' `dpmadev` CLI commands, which include commands that issue configuration tokens and send messages to phones.  Never enable the feature in a production build.  The feature is off by default.
:::
