---
title: "Administration"
---

## Authentication

### Configuration authentication

A phone's configuration contains the SIP password of every line in the phone section and the section's PIN.  With the default, `config_auth=disabled`, the server gives a phone section's configuration to any phone that completes the handshake and names the section.  On a production server, choose another value:

| Site | Setting | Result |
| --- | --- | --- |
| Each phone belongs to one desk. | `config_auth=mac`, with `mac` in every phone section | A phone gets only the section whose `mac` matches, and nobody types a PIN. |
| Each phone belongs to one desk, but MAC addresses are not trusted. | `config_auth=mac_pin` or `mac_globalpin` | The user must also know a PIN. |
| Users move between phones. | `config_auth=pin`, with `pin` in every phone section | The PIN identifies the user at any phone. |

#### Configuration tokens

A phone that passes the `config_auth` check receives a configuration token and from then on presents the token instead of the PIN, e.g., at every start.  The token contains two values computed from the phone section's name, the phone's MAC address, and a key that only the server has, so a section name and a MAC address are not enough to forge a token.

The server accepts a token without those values in two cases:

- **The phone section has never been issued a token with the values,** because the phone holds a token from the original module.  Like the original module, the server then accepts any token that begins with the section's name and the MAC address last seen for the section.  The exception ends the next time a phone requests a token for the section, e.g., at a user login or after a factory reset.  `database show OpenDPMATokens` lists the sections without the exception as `issued/<section>`.
- **The Asterisk database was lost.**  Until a phone requests a token for a section, the server accepts, like the original module, any token that begins with the section's name.

Both exceptions exist because stock firmware does not request a new token after a refusal: the phone shows an error and waits for someone to press **Retry**.

### User list

With `userlist_auth=disabled`, any phone that completes the handshake gets the list of all phone sections, with each section's name and extension.  With `userlist_auth=globalpin`, the phone must send `globalpin` or a `group_pin`, and a `group_pin` reveals only the sections with that `group_pin`.

Set `userlist_auth=globalpin` unless everyone on the phones' network may see the list of users.  On a shared server, give each tenant a `group_pin`.

### File directory

A phone that has completed the handshake can request any file under `file_directory`, including subdirectories, without a configuration token and regardless of `config_auth`.  Names that escape the directory with `..` are refused.

Put only files that every phone may read in the directory.  A `config_file` document must be in the directory, so any phone can request the document.

### Authentication incidents

The module records these failures by source address:

- a handshake with a certificate that fails verification,
- a user list request with a wrong PIN, and
- a configuration token request with a wrong PIN.  The reason is `invalid pin` for a section's `pin`, and `invalid config pass` for `globalpin` or a `group_pin`.

A record expires 600 seconds after the last failure.  To review the records:

- The log, at the `DPMA` level: `Auth failed for ip '<address>' mac '<MAC>' reason '<reason>' count=<n>`.  From the fifth failure from an address on, the line ends in `ALERT`.
- The AMI action `DPMAShowIncidents` lists the records; `DPMARemoveIncident` with the `IPAddress` header removes one.

:::note[The module does not block addresses]
The module keeps answering an address that the list shows with `Blocking: Yes`.  To block the address, feed the log line or the AMI list to the server's firewall.
:::

### Web interface and SSH

- `web_ui_enabled` is off by default.  When you turn the web interface on, set `login_password` in the same phone section, or the phone keeps a default password.
- On Open Desk Phone, the phone's web interface cannot change a `opendeskphone_*` option set on the server; without the option, the user decides.  To keep users from turning SSH on, set `opendeskphone_ssh_enabled=no` or leave `web_ui_enabled` off.

## Configuration changes

`module reload res_open_dpma.so` rereads `res_digium_phone.conf`.  A reload notifies running phones only when a phone section's lines changed, when the voicemail password behind `pin=voicemail` changed, or when a phone section was removed.  For any other change, reconfigure the phones:

| Interface | Command or action | Effect |
| --- | --- | --- |
| CLI | `digium_phones reconfigure phone <name>` | Tells one phone to fetch the configuration. |
| CLI | `digium_phones reconfigure all` | Tells every phone to fetch the configuration. |
| AMI | `DigiumPhoneReconfigure` with the `PhoneName` header | As the first command.  Requires the `message` authority. |
| AMI | `DigiumPhoneReconfigureAll` | As the second command.  Requires the `message` authority. |

Changes in `pjsip.conf` also require a reconfigure: the module picks up a changed endpoint, `auth` section, or AOR immediately but does not notify the phone.  After a SIP password change, a phone that is not reconfigured keeps registering with the old password.

Reconfiguring a phone whose address the server does not know fails with `Phone ... has not contacted the server`; the phone gets the new configuration at the next start.

`digium_phones reconfigure all` is cheap: a phone downloads the configuration only after a change.

## Firmware

### Update criteria

Only the first firmware entry for each model applies.

| Phone firmware | The phone installs the image when |
| --- | --- |
| Stock firmware | The entry's `version` text differs from the running version. |
| Open Desk Phone | The image differs from the running image.  The phone compares the checksums and sizes of the kernel and root file system, and treats `version` as a label. |

The module includes the image's checksum in the entry when the image file is on the server: in `file_directory` for a `firmware` section, or in the package directory for the firmware package.  Without the checksum, an Open Desk Phone phone reads the first few kilobytes of the image from the URL.  Keep the images there, under the file names in the entries, so that a phone running the image needs no download.

The module does not serve the images.  A web server must serve the directory at the network's `file_url_prefix`.

### Migration between stock firmware and Open Desk Phone {#firmware-migration}

A firmware entry can move phones from stock firmware to Open Desk Phone and back.  To move a server's D65 phones to Open Desk Phone, add an entry for the release's Open Desk Phone image:

```ini title="res_digium_phone.conf"
[fw-d65]
type=firmware
model=D65
version=3_0_0
file=3_0_0_D65_firmware.eff

[opendeskphone](!)
type=phone
firmware=fw-d65
```

To move the phones back, set `version` and `file` to the stock image.  Then [reconfigure](#configuration-changes) the phones, or wait for each phone to restart.

An Open Desk Phone phone does not retry an image that failed to install or was rolled back until the entry names a different image.  Such an image may explain a phone that still runs the previous firmware after a firmware change.  To inspect the phone's firmware banks, see [`dphone update status`](../dev-guide/firmware-updates.mdx#dphone-update-status).

:::note[Migrating the Digium D80 from stock firmware to Open Desk Phone]
Migrating the Digium D40 thru D70 can from stock firmware to Open Desk Phone is
straightforward, as the firmware package is fully compatible with the built-in
updater in the stock firmware.

However, with the Digium D80 (which the stock firmware is based on Android),
it's a little more complicated, as a new recovery partition needs flashed on
the phone before the firmware is downloaded.  Open DPMA detects stock firmware
migrating to Open Desk Phone, and handles this automatically using the phone's
built-in web server.  However, this means that the phone's IP address (at TCP
port 80) must be reachable from the Asterisk server.

In case you want to disable the migration check in Open DPMA, add
`d80_stock_firmware_migration=no` to the phone's config.
:::

## App data

Apps on Open Desk Phone can store data on the server, e.g., a game's best times, shown on every phone, or a user's progress, which survives a factory reset.  App data needs no configuration: the module answers the phones' app data requests and stores the data in the `OpenDeskPhoneAppData` family of the Asterisk database.

Each app has shared keys, which every phone in a store can access, and per-user keys.  The user is the phone section that the phone is provisioned as.  The firmware stores the user's theme as the per-user key `theme` of the reserved app name `opendeskphone`.

### Multi-tenant servers

Without [`opendeskphone_app_data_tenant`](opendeskphone-options.md#opendeskphone_app_data_tenant), all phones on the server share one store, so one tenant's phones see the names and scores that another tenant's users wrote.  On a multi-tenant server, set a key in each tenant's template:

```ini title="res_digium_phone.conf"
[north-office](!)
type=phone
opendeskphone_app_data_tenant=north-office

[phone101](north-office)
line=101
```

- Give every phone section a key.  A section without the option uses the store shared by all phones without a key.
- Check each new section with `digium_phones show phone <name>`.  An invalid key leaves the phone without app data, and the log reports the option as invalid.
- Avoid renaming tenant keys and phone sections.  Data stays under the key and section name in effect when the data was written: a phone with a new key sees a different store, and a renamed section loses the user's keys.

Two other lists cross tenant boundaries regardless of tenant keys:

- The user list, unless `userlist_auth=globalpin` and a `group_pin` per tenant restrict the list.
- The module's contact list of internal extensions, which includes every phone section with a line.

### Database layout

| Key in `OpenDeskPhoneAppData` | Contents |
| --- | --- |
| `@<tenant>/<app>/s/<key>` | An app's shared entry.  A bare `@`, without a tenant, is the store for phones without a key. |
| `@<tenant>/<app>/u/<phone section>/<key>` | One user's entry. |
| `watch/<phone section>` | The apps whose changes the phone is notified of. |
| `seq` | The last version number that the module assigned. |

In a phone section's name, each character other than a letter, a digit, `.`, or `-` is written as `=` and two hexadecimal digits.

| Task | Command |
| --- | --- |
| List all app data. | `database show OpenDeskPhoneAppData` |
| Delete one app's data for one tenant. | `database deltree OpenDeskPhoneAppData @north-office/lightup` |
| Delete one user's data for one app, in the store for phones without a key. | `database deltree OpenDeskPhoneAppData @/lightup/u/phone101` |
| Delete one tenant's data. | `database deltree OpenDeskPhoneAppData @north-office` |

- Removing a phone section from the configuration file keeps the section's app data.  Run `database deltree` when a user leaves.
- Within a tenant, `database deltree` on an app name with `_` also matches app names of the same length that differ only at that position, e.g., `@north-office/word_game` also matches `@north-office/word.game`.
- Do not delete `seq`, the counter for the version numbers that the module assigns.

## Monitoring

| Interface | Command or action | Output |
| --- | --- | --- |
| CLI | `digium_phones show sessions`{ .nowrap } | Each session: the age, the seconds since the last message, the phone's URI, whether the phone is authenticated and configured, and the MAC address. |
| CLI | `digium_phones show tokens`{ .nowrap } | Each phone section that a phone has claimed: the MAC address, the URI, and the session. |
| CLI | `digium_phones show phone <name>`{ .nowrap } | The options that the module holds for a phone section. |
| CLI | `digium_phones show status <name>`{ .nowrap } | The presence states that the phone offers, with the current state marked. |
| AMI | `DPMAShowPhones` | One `DPMAShowPhonesList` event per session, with the fields of `show sessions`, the user agent (the phone's model and firmware version), and the phone section.  Requires the `call` authority. |

Poll `DPMAShowPhones` from a monitoring system.  If a phone section that should have a phone has no session with `Configured: 1`, the phone is off, unreachable, or refused.

## Backups

The module stores state in four families of the Asterisk database:

| Family | Contents | If lost |
| --- | --- | --- |
| `OpenDeskPhoneAppData` | The app data. | The data is gone.  No other copy exists. |
| `DigiumPhoneSessions` | The phones' sessions, each with a session password. | Each phone starts a new session with a handshake. |
| `DigiumPhoneUsers` | For each phone section: the MAC address, the URI, and the session of the phone that claimed the section. | The server cannot reconfigure a phone until the phone contacts the server again. |
| `OpenDPMATokens` | The key for [configuration tokens](#configuration-tokens), and the phone sections issued a token made with the key. | The phones keep working.  For each phone section, the server accepts a token without the key's values until a phone requests a token for the section. |

- **Treat database backups as secrets.**  The session passwords in `DigiumPhoneSessions` are encrypted with a passphrase that is the same in every copy of Open DPMA and of the original module, which only prevents casual reading.  The key in `OpenDPMATokens` can mint a configuration token for any phone section.
- **Back up the module's files with the Asterisk configuration:** `res_digium_phone.conf`, which contains PINs and the passwords of external lines, the contents of `file_directory`, and the files named by `appserver_certificate` and `phone_ca_file`.

## Upgrades

1. Download the module from the new release and verify the checksum, as in [Download](installation.md#download).  Keep the file of the running version.
2. Replace `res_open_dpma.so` in the module directory.
3. Restart Asterisk.  `module reload`, and `module unload` followed by `module load`, keep the old code, which stays in the Asterisk process until the process exits.
4. Check `digium_phones show version` and the sessions, as in [Verification](installation.md#verification).

To roll back, restore the old file and restart Asterisk.

When you upgrade Asterisk to a new major version, install the module built for that version at the same time.
