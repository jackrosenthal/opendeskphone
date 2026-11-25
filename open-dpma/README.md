# Open DPMA

Open DPMA is an open-source, drop-in replacement for Digium's closed-source
Digium Phone Module for Asterisk (DPMA, `res_digium_phone`), written in
Rust.  Existing configurations and phones work with it unchanged.

It also fixes several bugs and security issues in the official DPMA, e.g.,
phone file requests that can read any file on the server, and inputs
that crash Asterisk.

Open DPMA is an independent rewrite, not made or endorsed by Digium or
Sangoma.

It is part of Open Desk Phone, the open firmware for the Digium D-series
phones, and has the firmware's version: each release of Open Desk Phone has
the firmware images and the modules.

## Installation

### Pre-built modules

The pre-built modules are for Asterisk 22, 23, and 24 on Linux, on x86_64
and aarch64, with glibc 2.34 or newer (e.g., Debian 12, Ubuntu 22.04, or
RHEL 9).  For anything else, see "Building".

| | x86_64 | aarch64 |
|---|---|---|
| Asterisk 22 | [res_open_dpma-asterisk22-x86_64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk22-x86_64.so) | [res_open_dpma-asterisk22-aarch64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk22-aarch64.so) |
| Asterisk 23 | [res_open_dpma-asterisk23-x86_64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk23-x86_64.so) | [res_open_dpma-asterisk23-aarch64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk23-aarch64.so) |
| Asterisk 24 | [res_open_dpma-asterisk24-x86_64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk24-x86_64.so) | [res_open_dpma-asterisk24-aarch64.so](https://github.com/jackrosenthal/opendeskphone/releases/latest/download/res_open_dpma-asterisk24-aarch64.so) |

### Installing the module

1. Copy the module into Asterisk's module directory (`astmoddir` in
   `asterisk.conf`) as `res_open_dpma.so`.
2. If you had the official DPMA already installed, remove or `noload` its
   three modules: `res_digium_phone.so`,
   `res_pjsip_endpoint_identifier_dpma.so`, and
   `res_pjsip_dialog_info_digium_body_supplement.so`.
3. Restart Asterisk.

A new build of the module also needs a restart of Asterisk: `module
reload`, and `module unload` followed by `module load`, keep the code
that the Asterisk process already has.

The module needs `res_pjsip`, `res_pjsip_session`, and `res_pjsip_pubsub`.
As with the official DPMA, `pjsip.conf` also needs the endpoint named by
`default_outbound_endpoint` in its `[global]` section; without it the
module fails to load, and Asterisk does not start.

### Log messages

As the official DPMA, the module writes most of its messages at a log
level of its own, `DPMA`, e.g., the reason a phone was refused a
configuration.  Asterisk shows them only in a log channel of `logger.conf`
that lists the level:

```
[logfiles]
console => notice,warning,error,dpma
messages.log => notice,warning,error,dpma
```

## Configuration

Configure it as the official DPMA, in `res_digium_phone.conf`; the
official DPMA's documentation and sample configuration apply.

These differ from the official DPMA:

- A `config_auth` value that is none of the values (an empty one
  included) makes the module refuse every request for a configuration
  token, with error -195 and no authentication incident, until the value
  is corrected and the module reloaded; an invalid `userlist_auth` value
  refuses every request for the user list the same way, with error -198.
  Each such line is logged as an error, and `digium_phones show settings`
  shows the value.  A phone that holds a token keeps getting the
  configuration, as with every valid value, and a session that a phone
  started while the value was invalid is checked with the corrected
  value.  The official DPMA ignores such a value without a message, which
  leaves authentication disabled.
- A reload that cannot read the file named by `phone_ca_file` or
  `appserver_certificate` (see "Additions"), or finds no CA certificate
  in the `phone_ca_file`, makes the module answer no phone handshake
  until a reload reads the file, as for a phone certificate that fails
  verification but without an authentication incident.  The module logs
  an error that names the option and the file.  Existing sessions are
  kept; a phone that handshakes, e.g., after a restart, is refused.
  Keeping the previous certificates would keep trusting a phone CA that
  the new file was meant to drop.  The official DPMA reads no such files.
- A configuration token is keyed: its two numbers are part of an HMAC
  of the phone section's name and the phone's MAC address, under a
  random key that the module keeps in AstDB (family `OpenDPMATokens`),
  and a phone section that was issued a keyed token accepts no other.
  The official DPMA's numbers are hashes of fixed texts, and it accepts
  every token that begins with a section's name and the MAC address last
  seen for the section, so that `config_auth` protects nothing from
  somebody who knows both.  A phone stores and returns the token without
  reading it, so phones see no difference.  A phone section that has
  not been issued a keyed token (its phone holds a token of the official
  DPMA) is treated as the official DPMA treats it, because stock
  firmware that is refused a token waits at an error screen for somebody
  to press a key.
- A wrong PIN in a request for a configuration token is recorded as an
  authentication incident (`DPMAShowIncidents`), as a wrong PIN in a
  request for the user list is.
- A text that has a `"`, a `<`, or an `&` that begins no reference, e.g.,
  in `idle_company_text`, is escaped in the phone's configuration.  The
  official DPMA writes it as it is, which makes a document that no phone
  can read.  A reference such as `&amp;` (written `&amp\;` in the file)
  stays as written.
- `enable_check_sync` is off again when its line is removed from the
  file.
- `network_vlan_qos` sets the RTP priority of a `type=network` section
  that has no `rtp_qos`.
- A `type=network` section without `registration_port` sends the phone
  the default port of the transport that is sent with it (the line's
  `transport`, or else the network's): 5061 for TLS, otherwise 5060.
  The official DPMA sends an empty port.
- The options of a `type=line` section go before the line's values from
  PJSIP at all times, also after a change in `pjsip.conf` or through
  sorcery.

### Additions

These `[general]` options are not in the official DPMA:

| Option | Meaning |
|---|---|
| `appserver_certificate=<file>` | A PEM file with the server certificate that the module sends to a phone in the handshake.  Default is the certificate of the official DPMA, which is built in. |
| `phone_ca_file=<file>` | A PEM file with the CA certificates against which the module verifies a phone's certificate.  Default is the CA certificates of the official DPMA, which are built in. |

When one of the files cannot be read, the module does not load, and a
reload refuses every phone handshake (see above).

### Additions for Open Desk Phone

These options in a phone section (`type=phone`) apply to phones running
Open Desk Phone.  The module sends each as a frozen setting
(`conflict="freeze"`): the phone uses the value of the configuration, and
a value set on the phone, e.g., in its web configuration, has no effect
while the option is there.  Stock firmware ignores the settings.

| Option | Meaning |
|---|---|
| `opendeskphone_ssh_enabled=yes\|no` | Whether the phone's SSH server runs.  Default is no. |
| `opendeskphone_ssh_authorized_key=<public key>` | An SSH public key that may log in to the phone, e.g., `ssh-ed25519 AAAA... name`.  Repeat it for more keys. |
| `opendeskphone_metrics_enabled=yes\|no` | Whether the phone serves `/metrics`.  Default is yes. |
| `opendeskphone_serial_console=yes\|no` | Whether the phone's serial port is a console.  Default is no. |
| `opendeskphone_static_address=<address>/<prefix length>` | The phone's static IPv4 address, e.g., `192.0.2.20/24`.  Default is DHCP. |
| `opendeskphone_static_gateway=<address>` | The gateway with the static address. |
| `opendeskphone_static_dns=<address>[,<address>...]` | The DNS servers with the static address. |
| `opendeskphone_disabled_apps=<app>[,<app>...]` | The built-in apps the phone does not offer, e.g., `calendar,parking`.  Default is none. |
| `opendeskphone_idle_softkeys=<softkey>[,<softkey>...]` | The softkeys of the idle screen, in order: `call_log`, `contacts`, `status`, `menu`, `apps`, `dnd`, `forward`, `redial`, `theme` (the theme selector of the color models D60, D62, D65, and D80; the other models leave it out), `voicemail` (what the Messages key does), or `app:<app>` for an app, e.g., `call_log,contacts,app:parking,menu`.  Default is `call_log,contacts,voicemail,redial,dnd,forward,apps,menu` on the D80.  On the other models, it is `call_log,contacts,status,menu,apps,dnd,forward`, and `theme` after them on the color models. |
| `opendeskphone_theme=<name>` | The built-in theme of a color model (D60, D62, D65, or D80): the wallpaper and the colors of the phone's screens, e.g., `dusk`.  The phone's theme selector lists the names.  A `wallpaper_file` goes before the theme.  Default is the phone's theme. |
| `opendeskphone_palette_file=<file>` | The palette file for the wallpaper of `wallpaper_file`: the colors of the phone's screens, as Open Desk Phone's `palette.proto` describes, e.g., `lobby.txtpb`.  Default is the phone's own palette. |
| `opendeskphone_ignore_provisioned_firmware=yes\|no` | Whether the phone ignores the firmware this configuration offers it.  Default is no. |
| `opendeskphone_app_data_tenant=<key>` | The tenant whose app data the phone reads and writes (see "App data"): 1 to 32 lower-case letters, digits, `.`, or `-`.  Default is none: the phone shares the app data of the phones without a key. |

An invalid value of `opendeskphone_ssh_enabled`, `opendeskphone_metrics_enabled`, or
`opendeskphone_serial_console` sends `no`, and is logged as an error; without a
value, the phone's user would decide.

The official DPMA's `web_ui_enabled=yes|no` controls the phone's web
configuration.  Default is no.

For example:

```
[phone101]
type=phone
opendeskphone_ssh_enabled=yes
opendeskphone_ssh_authorized_key=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa alice@laptop
```

### App data

The apps of Open Desk Phone phones can keep data on the server, e.g., a
game's best times, which every phone then shows, or a user's progress,
which comes back after the phone is reset.  There is nothing to turn on:
the module answers the phones' requests (`AppDataRequest`, which the
official DPMA does not have) and keeps the data in AstDB, in the family
`OpenDeskPhoneAppData`, so it is there after Asterisk restarts.

Each app has keys that all phones share, and keys for each user.  The
user is the phone section (`type=phone`) the phone is provisioned as,
shown to others by its `full_name`.

The phones keep one thing of their own there, under the name `opendeskphone`,
which no app can have: the theme a user chose on a color model (the
user's key `theme`).  A phone that was reset, and another phone of the
same user, take the theme from it.

On a server with more than one tenant, give each tenant's phones a
tenant key with `opendeskphone_app_data_tenant`.  Phones with the same key
share one store, and no phone reads, writes, or hears of the data of
another key; the phones without a key share a store of their own.  The
key is the server's alone: it is never sent to a phone, and no request
can name one.  A template sets it for all phones of a tenant:

```
[north-office](!)
type=phone
opendeskphone_app_data_tenant=north-office

[phone101](north-office)
line=101

[phone102](north-office)
line=102
```

A phone whose `opendeskphone_app_data_tenant` is not a valid key has no app
data until that is corrected.  The data stays with the key it was
written under: a phone that gets another key sees the other store, and
its user's keys there.

An app has at most 1024 shared keys and 64 keys for each user, a key is
at most 64 characters, and a value at most 4096 bytes.

`database show OpenDeskPhoneAppData` lists the data, and `database deltree`
removes it, e.g., `database deltree OpenDeskPhoneAppData @north-office/lightup`
for one app of one tenant (`@` alone is the phones without a key).  A
tenant key has no upper-case letter and no `_`, because `deltree`
matches without regard to case and reads `_` as any one character: no
`deltree` of one tenant removes the data of another.  The
rustdoc of `dpma_proto::appdata` describes the requests, and that of
`dpma_proto::appdata::store` the AstDB keys.

## Development

This directory, `open-dpma/`, is Open DPMA in the Open Desk Phone repository:

- `crates/`: the module (`res_open_dpma`) and its libraries.  The firmware
  uses two of them, `dpma-proto` and `dpma-bytes`.
- `tests/`: the test suites and their tools; see `tests/README.md`.
- `tools/`: `cargo xtask`, and the test image for each pinned Asterisk
  release.

The crates are members of the repository's one Cargo workspace, and take
their version from it.  Run the commands below in `open-dpma/`: `cargo xtask`
is Open DPMA's there (`.cargo/config.toml`), and the firmware's
anywhere else in the repository.  `target/` is the repository's.

### Building

You need Rust 1.99 or newer, libclang, and the configured headers of the
Asterisk version you build for, in `ASTERISK_SOURCE_DIR`.  `cargo xtask
headers` copies them out of a test image (podman or docker needed).

```
export ASTERISK_SOURCE_DIR=$(cargo xtask headers --asterisk 24)
cargo build --profile module -p res_open_dpma
```

The module is `target/module/libres_open_dpma.so`, in the repository's
`target/`.  Never enable the `devtest` feature in production; it adds CLI
commands for the test suites.

### Testing

```
cargo test -p dpma-bytes -p dpma-proto -p dpma-tests -p dpma-xtask
cargo xtask test [--asterisk 22] [all]
cargo xtask ci --asterisk 23
```

The suites run in containers (podman or docker).  `cargo xtask ci` is
what each CI job runs.

### Releases

CI runs `cargo xtask ci` for each pinned Asterisk major version on x86_64
and aarch64.  Pushing a tag `vX.Y.Z` matching the workspace version
publishes a GitHub Release of the firmware images and the modules.
