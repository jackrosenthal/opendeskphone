---
title: "Open Desk Phone options"
---

The `opendeskphone_*` options of a phone section (`type=phone`) configure features that only Open Desk Phone has.  Stock firmware ignores the options, so one phone section serves a phone before and after a firmware change.  The original module reports each option as invalid.

Example template:

```ini title="res_digium_phone.conf"
[opendeskphone](!)
type=phone
opendeskphone_ssh_enabled=yes
opendeskphone_ssh_authorized_key=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa alice@laptop
opendeskphone_disabled_apps=boxpusher,desksparrow,lightup
opendeskphone_app_data_tenant=north-office

[phone101](opendeskphone)
mac=000fd3000001
line=101
```

`digium_phones show phone <name>` shows the values that the module holds for a phone section.

## Precedence

- **A server value overrides the phone setting.**  The on/off options, the softkeys, the disabled apps, the theme, the palette file, and the static address are frozen settings: the phone uses the server's value, and the phone's web interface shows the setting as a disabled control labeled "Configured via DPMA".  Removing the option restores the value set on the phone.
- **An omitted option sends nothing.**  The phone uses the value set on the phone, or else the default listed below.
- **An invalid value is skipped.**  The module logs `Invalid option '<name>' at line <n> is not supported by phone module.` at the `DPMA` level and sends nothing for the option.  Exception: an invalid value of `opendeskphone_ssh_enabled`, `opendeskphone_serial_console`, or `opendeskphone_metrics_enabled` sends `no`, and the module also logs an error.

## Remote access

### `opendeskphone_ssh_enabled`

Turns the phone's SSH server on or off.

- **Value:** on/off.
- **Default:** off.

The SSH server gives a root shell and accepts public keys only.  Nobody can log in until the phone has a key, from [`opendeskphone_ssh_authorized_key`](#opendeskphone_ssh_authorized_key) or added on the phone.

### `opendeskphone_ssh_authorized_key`

An SSH public key that may log in to the phone.

- **Value:** one key in `authorized_keys` format, `<type> <base64 key> [comment]`, e.g., `ssh-ed25519 AAAA... alice@laptop`.  The type must begin with `ssh-`, `ecdsa-sha2-`, or `sk-`.  A line with `authorized_keys` options, e.g., `command="..."`, or longer than 8192 bytes is rejected.
- **Repeatable:** each occurrence adds a key.  Duplicates are sent once.
- **Default:** no keys.

The phone accepts these keys in addition to keys added on the phone, and drops a removed key at the next configuration fetch.

### `opendeskphone_serial_console`

Makes the serial port on the EHS connector a console.

- **Value:** on/off.
- **Default:** off.

The console and an electronic hook switch (EHS) headset adapter share the port.  With the console on, the phone cannot use an EHS adapter, and `ehs` has no effect.

### `opendeskphone_metrics_enabled`

Turns the phone's Prometheus endpoint, `GET /metrics`, on or off.

- **Value:** on/off.
- **Default:** on.

The endpoint requires no login and does not depend on `web_ui_enabled`.  Set `opendeskphone_metrics_enabled=no` to keep the metrics unreadable from the phones' network.

## Network address

The module sends `opendeskphone_static_gateway` and `opendeskphone_static_dns` only with `opendeskphone_static_address`.

:::caution[Check the address before reconfiguring the phone]
The server's address replaces a static address set on the phone.  A phone with a wrong address, prefix length, or gateway cannot reach the server to fetch a corrected configuration.
:::

### `opendeskphone_static_address`

The phone's IPv4 address and prefix length.

- **Value:** `<address>/<prefix length>`, e.g., `192.0.2.20/24`.  The prefix length is 1 to 30.  The address must be a host address: not the network or broadcast address of the prefix, and not a loopback, multicast, or reserved address.
- **Default:** DHCP.

### `opendeskphone_static_gateway`

The default gateway for the static address.

- **Value:** an IPv4 address, e.g., `192.0.2.1`.
- **Default:** no gateway.

### `opendeskphone_static_dns`

The DNS servers for the static address.

- **Value:** IPv4 addresses, separated by commas, e.g., `192.0.2.53,198.51.100.53`.  A line with an invalid address is skipped.
- **Repeatable:** each occurrence adds servers.  Duplicates are sent once.
- **Default:** no DNS server.  `registration_address` and the URL prefixes must then use addresses, not host names.

## Display and apps

### `opendeskphone_idle_softkeys`

The idle screen softkeys, from the left.

- **Value:** softkeys, separated by commas: functions from the table below, or `app:<name>` to open an app.  A line with an unknown softkey is skipped.  A phone provisioned by another server skips an unknown softkey and keeps the rest of the list.
- **Repeatable:** each occurrence appends softkeys.  A softkey listed twice appears once.
- **Default:** `call_log,contacts,voicemail,redial,dnd,forward,apps,menu` on the D80.  On the other models, `call_log,contacts,status,menu,apps,dnd,forward`, then `theme` on the color models (D60, D62, and D65).

| Function | Softkey |
| --- | --- |
| `call_log` | The call log.  Shows missed calls while unseen missed calls exist. |
| `contacts` | The Contacts app. |
| `status` | Information about the phone. |
| `menu` | The menu. |
| `apps` | The list of apps. |
| `dnd` | Toggles Do Not Disturb. |
| `forward` | The call forwarding menu. |
| `redial` | Redial. |
| `theme` | The theme selector.  Color models only. |
| `voicemail` | Voicemail, as the Messages key opens it: the Voicemail app when the app has its account on the server, or else a call to the voicemail number. |

Use `voicemail` rather than `app:voicemail`, which opens the Voicemail app even when the app has no account to reach the server with.

Example:

```ini
opendeskphone_idle_softkeys=call_log,contacts,app:parking,menu
```

The stock firmware's idle screen options (`show_call_log`, `can_forward_calls`, `show_call_parking`, and a custom application's `idlescreen_softkey`) do not affect Open Desk Phone's idle softkeys.

### `opendeskphone_disabled_apps`

Built-in apps to hide.  A disabled app is missing from the list of apps and cannot be opened.

- **Value:** app names, separated by commas, e.g., `calendar,parking`.  Names contain letters, digits, `_`, `-`, and `.`.  A line with an invalid name is skipped.
- **Repeatable:** each occurrence adds names.
- **Default:** none.

Built-in app names: `alarms`, `boxpusher`, `calendar`, `contacts`, `desksparrow`, `diagnostics`, `hotdesking`, `lightup`, `parking`, `queues`, `status`, `timer`, and `voicemail`.  Disable apps for services that the server lacks, e.g., `queues` without `app_queue`.

### `opendeskphone_theme`

The built-in theme: a wallpaper and the color palette for every screen.

- **Value:** a theme name, e.g., `dusk`: 1 to 32 letters, digits, `_`, and `-`.  The phone's theme selector and web interface list the themes.  For an unknown name, the phone shows the default theme.
- **Default:** the theme chosen on the phone, or else Tides.
- **Models:** D60, D62, D65, and D80.

A `wallpaper_file` in the phone section replaces the theme.

### `opendeskphone_palette_file`

The palette for the `wallpaper_file` picture: the colors of every screen while that picture is the wallpaper.

- **Value:** a file name or URL, e.g., `lobby.txtpb`, located like `wallpaper_file`: the module serves a file in `file_directory`; another name is fetched from the network's `file_url_prefix`; a full URL is used as written.
- **Default:** the phone's current palette.
- **Models:** D60, D62, D65, and D80.

The phone fetches the palette only with `wallpaper_file`.  The file is a `opendeskphone.palette.v1` message in protobuf text format, at most 16 KiB, defined in `proto/opendeskphone/palette/v1/palette.proto` in the repository.

## Firmware

### `opendeskphone_ignore_provisioned_firmware`

Makes the phone ignore the configuration's firmware entries, e.g., to keep a development phone on a test build.

- **Value:** on/off.
- **Default:** off: the phone installs the image of the matching entry when that image differs from the running firmware.

See [Firmware](administration.md#firmware).

## App data

### `opendeskphone_app_data_tenant`

The tenant whose app data the phone reads and writes.

- **Value:** a key of 1 to 32 lowercase letters, digits, `.`, and `-`.  An uppercase letter or `_` makes the value invalid.
- **Default:** the store shared by all phones without a key.

Each key has a separate store, shared by the phones with that key.  The module never sends the key to a phone, and a phone's request cannot name a key.

An invalid value does not mean "no key": the module logs the option as invalid and refuses the phone's app data requests until the value is fixed, so a typo never puts a tenant's phone in the shared store.

For the store and multi-tenant servers, see [App data](administration.md#app-data).
