---
title: "Configuration reference"
---

Open DPMA reads `res_digium_phone.conf` from the Asterisk configuration directory.

## File format

The file has a `[general]` section and any number of sections with a `type` option.  A section's name identifies the phone, line, network, or other object.

- **On/off options** accept any value that Asterisk reads as true (`yes`, `true`, `on`, `1`) or false.
- **Errors.**  An unknown option or an unreadable value is logged at the `DPMA` level and skipped; the rest of the section applies.  A section with a missing or unknown `type` is logged and skipped.  Exceptions: an invalid [`config_auth`](#config_auth) or `userlist_auth` value refuses requests, and an invalid `opendeskphone_ssh_enabled`, `opendeskphone_serial_console`, or `opendeskphone_metrics_enabled` value sends `no`; each is also logged as an error.
- **Special characters.**  The module escapes `"`, `<`, and `&` in the phone's XML configuration, so write these characters as the phone should display them.  An `&` that begins a reference, e.g., `&amp;` or `&#38;`, is left alone, so values written for the original module, which escapes nothing, display the same.
- **Comments.**  `;` begins a comment; `\;` is a literal semicolon.

A reload (`module reload res_open_dpma.so`) does not reconfigure running phones, with the exceptions listed in [Configuration changes](administration.md#configuration-changes).

## Minimal example

```ini title="res_digium_phone.conf"
[general]
config_auth=mac
registration_address=192.0.2.10
registration_port=5060

[phone101]
type=phone
mac=000fd3000001
line=101
```

`registration_address` gives every phone a network.  A phone without a network gets no SIP accounts.

## `[general]`

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `config_auth` | `disabled`, `mac`, `pin`, `globalpin`, `mac_pin`, or `mac_globalpin` | `disabled` | What a phone must prove to get a phone section's configuration.  See [`config_auth`](#config_auth). |
| `userlist_auth` | `disabled` or `globalpin` | `disabled` | With `globalpin`, a phone must send `globalpin` to get the list of all phone sections, or a `group_pin` to get the sections with that `group_pin`.  A request that matches no section is refused and recorded as an authentication incident. |
| `globalpin` | Text | None | The global PIN for `config_auth` and `userlist_auth`. |
| `registration_address` | Host name or address | None | This server's address for phones.  Gives every phone a default network: named after the phone section, with the alias `Default Network`, matching every address (`0.0.0.0/0`), and listed after the phone's `network` options. |
| `registration_port` | Port | `5060` | The default network's SIP port. |
| `firmware_url_prefix` | URL | None | The default network's `file_url_prefix`: the URL where a web server serves `file_directory`.  A missing trailing `/` is added.  Without `registration_address`, also creates the default network. |
| `file_directory` | Directory | `digium_phones` in Asterisk's `astvarlibdir`, e.g., `/var/lib/asterisk/digium_phones` | The files named in the configuration: contact lists, logos, wallpapers, palette files, display rules, firmware images, and `config_file` files.  The module serves contact lists from this directory. |
| `firmware_package_directory` | Directory | None | A directory with firmware images and a `digium_phones_firmware.conf` that lists the images.  See [Firmware package](#firmware-package). |
| `service_discovery_enabled` | On/off | `yes` | Announces the server over mDNS as `_digiumproxy._udp`, so phones on the same network segment find the server without an address. |
| `service_name` | UTF-8 text, at most 63 bytes | `Digium Phone Config Server` | The announced service name, shown in the phones' list of servers.  A longer or invalid name is not announced. |
| `mdns_address` | Host name or address | The value of `registration_address` | The server address in the announcement.  Without an address, the announcement has no SIP URL. |
| `mdns_port` | Port | The value of `registration_port`, or else `5060` | The port in the announcement. |
| `mdns_transport` | Text, e.g., `tcp` | None | Appended to the announced SIP URL as `;transport=<value>`. |
| `server_uuid` | Text | None | A server identifier, sent with every internal line account and app account. |
| `pjsip_message_context` | Dialplan context name | `dpma_pjsip_message_context` | The dialplan context that the module creates for messages from phones.  Change only if another context uses the name. |
| `transaction_initial_threads` | Number, at least 1 | `1` | Initial number of threads for phone requests.  Capped at `transaction_max_threads`. |
| `transaction_max_threads` | Number, at least 1 | `4` | Maximum number of threads for phone requests. |
| `message_delay` | Milliseconds | `5` | Delay between the SIP messages of a multi-message reply. |
| `appserver_certificate` | Path to a PEM file | A certificate built into the module | The server certificate sent to phones in the handshake.  Open DPMA only. |
| `phone_ca_file` | Path to a PEM file with one or more CA certificates | The CA certificates built into the module | The CA certificates that verify a phone's certificate in the handshake.  Open DPMA only. |
| `d80_stock_firmware_migration` | On/off | `yes` | [Re-keys](administration.md#firmware-migration) a D80 that contacts the server running stock firmware, when the firmware package contains an Open Desk Phone image for it.  Open DPMA only. |

The built-in certificates are the original module's, which D-series phones trust.  If the file named by `appserver_certificate` or `phone_ca_file` is unreadable, the module does not load at startup.

:::caution[An unreadable certificate file refuses handshakes]
After a reload that cannot read the file named by `appserver_certificate` or `phone_ca_file`, the module answers no phone handshake until a reload reads the file.  Phones keep established sessions; a phone that handshakes, e.g., after a restart, gets no configuration.  The module logs an error, e.g., `Cannot read phone_ca_file '<file>': No such file or directory (os error 2): every phone handshake is refused until a reload reads the file`.
:::

### `config_auth`

| Value | The phone presents | The server checks |
| --- | --- | --- |
| `disabled` | The user chosen from the user list. | Nothing. |
| `mac` | The phone's MAC address.  The phone shows no user list. | A phone section has that `mac`. |
| `pin` | The chosen user and a PIN. | The PIN matches the section's `pin`. |
| `globalpin` | The chosen user and a PIN. | The PIN matches `globalpin` or the section's `group_pin`. |
| `mac_pin` | The MAC address, the chosen user, and a PIN. | The section has that `mac`, and the PIN matches the section's `pin`. |
| `mac_globalpin` | The MAC address, the chosen user, and a PIN. | The section has that `mac`, and the PIN matches `globalpin` or the section's `group_pin`. |

:::caution[A section without a PIN accepts an empty PIN]
A phone section without a PIN (`pin` missing or misspelled, or `pin=voicemail` with no voicemail password) accepts an empty PIN.  Under `config_auth=pin`, a phone that sends no PIN gets the section's configuration; under `mac_pin`, a phone that sends the section's MAC address and no PIN does.  Under every `config_auth` value, the section's app account accepts an empty password.  `userlist_auth=globalpin` with no `globalpin` gives the list of all phone sections to a phone that sends no PIN.
:::

:::caution[A misspelled value refuses requests]
A `config_auth` value not listed above, including an empty value, makes the module refuse every configuration token request until the value is corrected and the module reloaded.  An invalid `userlist_auth` value refuses every user list request the same way.  A phone without a token gets no configuration.  A phone with a token keeps getting the section's configuration, as with every listed value.  The module logs an error, e.g., `Invalid value '<value>' for config_auth at line <n> of res_digium_phone.conf: configuration token requests are refused ...`, and `digium_phones show settings` shows the value under `Config Authentication` or `Userlist Authentication`.  To choose a value, see [Authentication](administration.md#authentication).
:::

## `type=phone` {#phone}

A phone section describes one phone, or, when `config_auth` does not include `mac`, one user: any phone can take the section's configuration when the user picks the section from the user list.

A "Repeatable" option may appear more than once; each occurrence adds to a list.  The `opendeskphone_*` options are in [Open Desk Phone options](opendeskphone-options.md).

### Identity and access

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `mac` | MAC address, with or without colons | None | The section's phone, for the `config_auth` values with `mac`. |
| `pin` | Text, or `voicemail` | None | The section's PIN, for the `config_auth` values with `pin`, and the password of the phone's app account, which apps send with every request.  With `voicemail`, the PIN is the voicemail password of the primary line's mailbox in `voicemail.conf`, and a password change reconfigures the phone. |
| `group_pin` | Text | None | A PIN shared by several phone sections, e.g., one tenant's, and accepted instead of `globalpin` for this section.  See [`config_auth`](#config_auth) and [`userlist_auth`](#general). |
| `full_name` | Text | The section's name | Shown in the user list and in the module's contact list of internal extensions, and recorded as the author of app data. |
| `web_ui_enabled` | On/off | `no` | Enables the phone's web interface. |
| `login_password` | Text | Not sent | The web interface password. |
| `lock_preferences` | On/off | `no` | Hides the display, sound, answering, localization, and network selection menus, and makes the section's values override values set on the phone. |
| `logout_enabled` | On/off | `no` | Adds the Log Out app. |
| `config_file` | File name in `file_directory`{ .nowrap } | None | A complete configuration document to send instead of the generated one.  No other option in the section then affects the configuration. |

### Lines and linked sections

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `line` | Name of a PJSIP endpoint | None | Repeatable.  A line on this server.  The first `line` or `external_line` is the primary line, which serves the phone's voicemail, parking, and status, and supplies the extension in the user list.  At most 12 lines; more are dropped.  A [`type=line`](#line) section of the same name adds settings. |
| `external_line` | Name of a `type=external_line` section | None | Repeatable.  A line on another SIP server. |
| `network` | Name of a `type=network` section | None | Repeatable.  A network that the phone can be on.  The phone gets each account once per network and selects a network by `cidr`. |
| `firmware` | Name of a `type=firmware` section | None | Repeatable.  A firmware image.  See [Firmware](administration.md#firmware). |
| `application` | Name of a `type=application` section | None | Repeatable.  Settings for one app. |
| `alert` | Name of a `type=alert` section | None | Repeatable.  An alert. |
| `multicastpage` | Name of a `type=multicastpage` section | None | Repeatable.  A multicast page that the phone listens for. |
| `ringtone` | Name of a `type=ringtone` section, or a built-in ringtone | None | Repeatable.  A ringtone file to load onto the phone.  A built-in ringtone's name instead makes that ringtone active, unless `active_ringtone` appears earlier.  Built-in ringtones: `Alarm`, `Chimes`, `Digium`, `GuitarStrum`, `Jingle`, `Office`, `Office2`, `RotaryPhone`, `SteelDrum`, `Techno`, `Theme`, `Tweedle`, `Twinkle`, and `Vibe`. |
| `active_ringtone` | Name of a ringtone | `Digium` | The ringtone for normal calls. |
| `codecs` | Comma-separated codec names | Not sent | The phone's codecs, in order of preference; `!` before a name disables the codec.  Names: `PCMU`, `PCMA`, `G722`, `G7221`, `G726-32`, `opus`, `G729`, `iLBC`, `L16`, and `L16-256`.  An unknown name ends the list. |

### Contacts and busy lamp fields

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `contact` | File name or URL | An empty list | Repeatable.  A contact list.  The module serves a file in `file_directory`; another name is fetched from the network's `file_url_prefix`; a full URL is used as written. |
| `blf_contact_group` | Name of a contact group | Not sent | The contact group for busy lamp field keys. |
| `contacts_display_rules` | File name in `file_directory`{ .nowrap } | None | An XML file of display rules for the Contacts app. |
| `contacts_max_subscriptions` | Number | Not sent | Maximum number of contacts whose state the phone subscribes to. |
| `blf_items` | File name or URL | None | A file that assigns contacts to line keys, fetched from the network's `file_url_prefix`. |
| `blf_unused_linekeys` | On/off | `no` | Shows contacts on unused line keys. |
| `blf_page_return_timeout` | Number | `0` | Time after which the line keys return to the first page. |
| `name_format` | Text | Not sent | Order of the parts of a contact's name. |
| `use_local_storage` | On/off | `no` | Lets the Contacts app store contacts on the phone. |

### Call features

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `send_to_vm` | On/off | `yes` | Enables the softkey that sends an incoming call to voicemail, and transfer to voicemail. |
| `can_forward_calls` | On/off | `yes` | Enables call forwarding. |
| `record_own_calls` | On/off | `no` | Enables the softkey that records the call. |
| `show_call_log` | On/off | `yes` | Enables the call log. |
| `show_call_parking` | On/off | `yes` | Enables the Parking app. |
| `parking_exten` | Extension | None | The parking extension.  Setting a value adds the Park softkey. |
| `parking_transfer_type` | `blind` or `attended` | Not sent | How the phone transfers a call to `parking_exten`. |
| `accept_local_calls` | Text | Not sent | Hosts that the phone accepts calls from. |
| `call_waiting_tone` | On/off | `yes` | Plays a tone when a second call arrives. |
| `headset_answer` | On/off | `no` | Answers calls on the headset. |
| `ring_headset_only` | On/off | `no` | Rings on the headset only. |
| `reset_call_volume` | On/off | `no` | Resets the call volume to the default after each call. |
| `ehs` | Text | Not sent | Type of electronic hook switch (EHS) headset adapter. |
| `enable_check_sync` | On/off | `no` | Makes the phone act on SIP `check-sync` notifications. |
| `transport_tls_allowed` | On/off | Not sent | Allows TLS for SIP. |
| `allow_insecure_ssl` | On/off | `no` | Makes the phone accept TLS certificates that fail verification. |
| `handshake_timeout` | Seconds | `946080000` (30 years) | Lifetime of the phone's handshake with the server. |

### Display and sound

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `timezone` | Time zone name, e.g., `America/Denver`{ .nowrap } | Not sent | Time zone. |
| `ntp_resync` | Seconds | Not sent | Interval between NTP queries. |
| `active_locale` | Locale name, e.g., `en_US` | Not sent | Language. |
| `brightness` | Number | Not sent | Display brightness. |
| `contrast` | Number | Not sent | Display contrast. |
| `dim_backlight` | On/off | `yes` | Dims the backlight when idle.  `backlight_timeout=0` turns dimming off. |
| `backlight_timeout` | Seconds | Not sent | Idle time before the backlight dims. |
| `backlight_dim_level` | Number | Not sent | Brightness of the dimmed backlight. |
| `default_fontsize` | Number | Not sent | Font size. |
| `small_clock` | On/off | `no` | Shows the small clock on the idle screen. |
| `idle_company_text` | Text | Not sent | Text on the idle screen. |
| `display_mc_notification` | On/off | `yes` | Shows a notification for missed calls. |
| `ringer_volume` | Number | Not sent | Ringer volume. |
| `speaker_volume`, `handset_volume`, `headset_volume` | Number | Not sent | Call volume of each output. |
| `handset_sidetone_db` | Decibels | `-25` | Handset sidetone level.  `0` keeps the phone's value. |
| `headset_sidetone_db` | Decibels | `-15` | Headset sidetone level.  `0` keeps the phone's value. |
| `<model>_logo_file` | File name or URL | None | A logo for one model, e.g., `d70_logo_file`.  The module serves a file in `file_directory`; another name is fetched from the network's `file_url_prefix`. |
| `wallpaper_file` | File name or URL | None | A wallpaper, located like a logo file. |
| `expansion_enabled` | On/off | `no` | Enables an expansion module. |
| `expansion_page_indicators_enabled` | On/off | `no` | Shows page indicators on the expansion module. |

With `lock_preferences=no`, display and sound values are initial values, and a value set on the phone takes precedence.

### Network and logging

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `ntp_server`, `syslog_server`, `syslog_port`, `syslog_level` | As in a [network section](#network) | None | Sets the option on the phone's default network (see `registration_address` in `[general]`).  Without `registration_address`, creates the default network without a server address. |
| `lan_port_mode`, `pc_port_mode` | Text | Not sent | Speed and duplex of the LAN port and PC port. |
| `8021x_method` | Text | Empty | 802.1X authentication method. |
| `8021x_identity`, `8021x_anonymous_identity`, `8021x_password` | Text | Empty | 802.1X credentials. |
| `8021x_client_cert_url`, `8021x_root_cert_url` | URL | Empty | Where the phone downloads the 802.1X client certificate and root certificate. |
| `8021x_client_cert_value`, `8021x_root_cert_value` | File name | The file name at the end of the URL | The file name under which the phone stores each certificate. |
| `8021x_passthrough` | Text | Empty | Whether the phone passes 802.1X frames to the PC port. |
| `8021x_eapol_on_disconnect` | Text | `0` | Whether the phone sends an EAPOL logoff when the PC port's device disconnects. |
| `8021x_debug` | Text | Not sent | 802.1X logging. |
| `openvpn_enable` | On/off | `no` | Connects the phone to an OpenVPN server. |
| `openvpn_config_file_url`, `openvpn_root_cert_url`, `openvpn_client_cert_url`, `openvpn_client_key_url` | URL | Empty | Where the phone downloads the OpenVPN configuration, root certificate, client certificate, and client key. |
| `openvpn_config_file_value`, `openvpn_root_cert_value`, `openvpn_client_cert_value`, `openvpn_client_key_value` | File name | The file name at the end of the URL | The file name under which the phone stores each file. |
| `openvpn_logging` | Text | Not sent | OpenVPN logging. |

The module passes "Text" values to the phone unchanged; the firmware defines which values are valid.

## `type=line` and `type=external_line` {#line}

A phone's `line` names a PJSIP endpoint on this server.  The module reads these settings from `pjsip.conf` and tracks later changes:

| Line setting | Source in `pjsip.conf` |
| --- | --- |
| Context and caller ID | The endpoint's `context` and `callerid`. |
| User name and password | The `username` and `password` of the endpoint's first `auth` section. |
| Mailbox | The endpoint's `mailboxes`, or else the `mailboxes` of the endpoint's first AOR that has the option.  Only the first mailbox is used. |

As with the original module, a `users.conf` user with `hasvoicemail` gives a mailbox to the line of the same name.

A `type=line` section named after the endpoint is optional and sets what PJSIP does not provide.  Options in the section override the PJSIP values, also after the endpoint changes.

Each external line requires a `type=external_line` section with all of the line's settings.

Both types accept these options:

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `exten` (or `extension`) | Extension | The line's name | The number that reaches the line, shown in the user list and in the module's contact list of internal extensions. |
| `line_label` | Text | The line's name | Line key label. |
| `callerid` | Caller ID | From PJSIP | The caller ID that the phone shows for the line. |
| `first_name`, `last_name`, `title`, `location` | Text | None | The line user's details, shown in the phone's account and in the module's contact list of internal extensions. |
| `digit_map` | Digit map | None | The dial plan that decides when a dialed number is complete. |
| `mailbox` | `mailbox@context`{ .nowrap } | From PJSIP or `users.conf` | The line's mailbox.  The context defaults to `default`.  Of a list, only the first mailbox is used. |
| `voicemail_uri` | SIP URI or number | None | A number that the phone calls for voicemail instead of opening the Voicemail app. |
| `context` | Dialplan context | `default`, or from PJSIP | The context of the line's hint when `subscribecontext` is not set. |
| `subscribecontext` | Dialplan context | The value of `context` | The context of the hint `auto_hint_<line>`, which other phones subscribe to for the line's state. |
| `transport` | `udp`, `tcp`, or `tls` | The network's `transport` | SIP transport to the primary server. |
| `media_encryption` | Text, e.g., `sdes` | Not sent | Media encryption with the primary server. |
| `secondary_server_transport` | `udp`, `tcp`, or `tls` | The network's `alternate_transport` | SIP transport to the alternate server. |
| `secondary_media_encryption` | Text | Not sent | Media encryption with the alternate server. |
| `reregistration_timeout` | Seconds | Not sent | Registration lifetime. |
| `registration_retry_interval` | Seconds | Not sent | Delay before the phone retries a failed registration. |
| `registration_max_retries` | Number | Not sent | Maximum retries of a failed registration. |
| `register` | On/off | `yes` | Whether the phone registers the line. |
| `host` | `dynamic`, or a host | `dynamic` | Any value other than `dynamic` keeps the phone from registering the line. |
| `outboundproxy_address` | Host name or address | None | Outbound proxy. |
| `outboundproxy_port` | Port | `5060` | Outbound proxy port. |
| `plar_number` | Number | None | A number that the phone dials when the line goes off hook. |
| `secret` | Text | From PJSIP | SIP password.  Required for an external line. |
| `authname` | Text | From PJSIP.  For an external line, `userid`, or else the line's name. | SIP authentication user name. |
| `userid` | Text | The line's name | External lines only: SIP user name. |
| `server_address` | Host name or address | None | External lines only: SIP server. |
| `server_port` | Port | `5060` | External lines only: SIP server port. |
| `secondary_server_address` | Host name or address | None | External lines only: alternate SIP server. |
| `secondary_server_port` | Port | `5060` | External lines only: alternate server port. |
| `tech` | Text | `PJSIP` for a line with a PJSIP endpoint | Channel technology of the line's hint.  Set by the module; do not set in the file. |
| `pjsip_auth` | Name of an `auth` section | The endpoint's first `auth` section | The `auth` section whose changes the line tracks.  Set by the module; do not set in the file. |

An internal line takes the server address, port, and alternate server from the phone's [networks](#network).

## `type=network` {#network}

A network section gives the server addresses that a phone uses while the phone's address is within `cidr`.  A phone that moves between networks, e.g., an office and a VPN, needs a section for each.

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `cidr` | Network in CIDR notation, e.g., `10.10.0.0/16` | None | Addresses for which the phone uses this network. |
| `alias` | Text | None | The network's name on the phone. |
| `registration_address` | Host name or address | None | This server's address on the network. |
| `registration_port` | Port | `5061` with TLS, otherwise `5060` | This server's SIP port.  The default follows the transport sent for the line: the line's `transport`, or else the network's. |
| `transport` | `udp`, `tcp`, or `tls` | None | SIP transport for lines without `transport`. |
| `alternate_registration_address` | Host name or address | None | A second server, for when the first does not answer. |
| `alternate_registration_port` | Port | None | The second server's SIP port. |
| `alternate_transport` | `udp`, `tcp`, or `tls` | None | SIP transport to the second server. |
| `file_url_prefix` | URL | None | The URL where a web server serves `file_directory` to this network.  A missing trailing `/` is added.  Ringtones, custom apps, `blf_items`, and firmware are offered only with this option. |
| `public_firmware_url_prefix` | URL | `http://dphone.dl.digium.com/firmware/asterisk/` | Where phones find the images of firmware sections with `public_firmware=yes`. |
| `ntp_server` | Host name or address | Not sent | NTP server. |
| `syslog_server` | Host name or address | None | Syslog server.  Used only with `syslog_level`. |
| `syslog_port` | Port | Not sent | Syslog server port. |
| `syslog_level` | Text, e.g., `error` | Not sent | Lowest level that the phone logs. |
| `network_vlan_discovery_mode` | Text, e.g., `MANUAL` | Not sent | How the phone finds the VLAN.  The three VLAN options below are sent only with this option. |
| `network_vlan_id` | VLAN number | Not sent | The phone's VLAN.  Sent when greater than `0`. |
| `sip_qos` | `0` to `7` | `0` | 802.1p priority of SIP packets on the VLAN. |
| `rtp_qos` | `0` to `7` | `0` | 802.1p priority of RTP packets on the VLAN. |
| `network_vlan_qos` | `0` to `7` | — | Deprecated name for `rtp_qos`; logged as deprecated.  Applies only if no `rtp_qos` appears earlier in the section. |
| `pc_vlan_id` | VLAN number | `0` | VLAN of the PC port. |
| `pc_qos` | Number | `0` | 802.1p priority of the PC port's packets. |
| `sip_dscp` | Number | `0` | DSCP value of SIP packets. |
| `rtp_dscp` | Number | `0` | DSCP value of RTP packets. |
| `udp_ka_interval` | Seconds | `0` | Interval between the phone's UDP keep-alive packets. |

## `type=firmware` {#firmware}

A firmware section describes one image.  A phone section lists images with `firmware`; only the first entry for each model applies.  For when a phone installs an image, see [Firmware](administration.md#firmware).

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `model` | Model name, e.g., `D65` | None | The image's phone model, converted to uppercase. |
| `version` | Version text, e.g., `3_0_0` | None | The image's version.  Stock firmware installs the image when this text differs from the running version. |
| `file` | File name or URL | None | The image.  A file name is fetched from the network's `file_url_prefix`; a full URL is used as written.  When the file is in `file_directory`, the entry includes the image's checksum. |
| `public_firmware` | On/off | `no` | Lets the phone also fetch the image from the network's `public_firmware_url_prefix`, as `<version>/<version>_<model>_firmware.eff`. |

An entry is sent only to a network with a `file_url_prefix`, and only for a section with both `version` and `file`.

### Firmware package

A phone section without `firmware` gets the images of the firmware package: the directory named by `firmware_package_directory`, with the images and a `digium_phones_firmware.conf` whose sections are named after phone models:

```ini title="digium_phones_firmware.conf"
[D65]
version=3_0_0
file=3_0_0_D65_firmware.eff
```

| Option | Value | Description |
| --- | --- | --- |
| `version` | Version text | As in a firmware section. |
| `file` | File name or URL | As in a firmware section.  The checksum comes from the file in the package directory. |

## `type=application` {#application}

An application section holds settings for the app that the `application` option selects.

### `application=queue`

Shows one `app_queue` queue in the Queues app, and logs the user in and out.

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `queue` | Queue name | None | The queue. |
| `member` | On/off | `yes` | Whether the user is a member.  With `no`, the user only monitors the queue. |
| `location` | Interface, e.g., `PJSIP/101` | None | The member interface for adding the user and for finding the user among the members. |
| `membername` | Text | None | The member name for adding the user. |
| `permission` (or `permissions`) | `status`, `overview`, or `details` | `status` | What the phone shows: `status`, the user's status; `overview`, also the number of members; `details`, also the logged-in members and the waiting callers. |
| `login_exten` | `extension@context`{ .nowrap } | None | An extension that the module calls from the phone to log the user in, instead of adding the member directly. |
| `logout_exten` | `extension@context`{ .nowrap } | None | An extension that the module calls from the phone to log the user out, instead of removing the member directly. |

### `application=status`

Adds one presence state to the Status app.  A phone with status sections offers only their states.

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `status` | `available`, `unavailable`, `away`, `xa`, `chat`, or `dnd` | None | The presence state.  Required. |
| `substatus` | Text | None | Repeatable.  A sub-state to choose with the state, e.g., `Out to lunch`. |
| `send486` | On/off | `yes` for `dnd` and `xa`, otherwise `no` | Whether the phone rejects calls as busy in this state. |

### `application=parking`

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `parkinglot` | Comma-separated parking lot names | None | Repeatable.  The parking lots that the Parking app shows.  Without a parking section, the app shows every lot. |

### `application=voicemail`

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `translation` | Name of a `type=translation` section | None | Display names for the voicemail folders. |
| `require_password` | On/off | `no` | Whether the Voicemail app asks for the voicemail password. |

### `application=custom`

A custom section loads an app package onto phones running stock firmware.  Open Desk Phone skips the package, because Open Desk Phone apps use another format; see the [App SDK](../app-sdk/index.md).

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `name` | Text | None | The app's identifier on the phone. |
| `filename` | File name or URL | None | The app package, fetched from the network's `file_url_prefix`. |
| `md5` (or `md5sum`) | MD5 digest | Not sent | The package's digest. |
| `autostart` | On/off | `no` | Starts the app with the phone. |
| `idlescreen_softkey` | On/off | `no` | Adds an idle screen softkey for the app. |
| Any other option | Text | — | Passed to the app as a setting of the same name. |

## `type=ringtone` {#ringtone}

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `filename` | File name or URL | None | The sound file, fetched from the network's `file_url_prefix`. |
| `alias` | Text | The section's name | The ringtone's name on the phone. |

## `type=alert` {#alert}

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `alert_info` | Text | None | The `Alert-Info` value that selects the alert.  Required. |
| `ring_type` | `normal`, `ring-answer`, `visual`, or `answer` | `normal` | How the phone alerts the user. |
| `ringtone` | Name of a ringtone | None | The alert's ringtone. |

Every phone also has four built-in alerts:

| `Alert-Info` | Ring type |
| --- | --- |
| `normal` | `normal` |
| `ring-answer` | `ring-answer` |
| `visual` | `visual` |
| `intercom` | `answer` |

## `type=multicastpage` {#multicastpage}

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `address` | Multicast address | None | The address that the phone listens on. |
| `port` | Port | None | The port that the phone listens on. |
| `alias` | Text | The section's name | The page's name on the phone. |
| `priority` | Number | None | Priority relative to other pages. |
| `interrupt` | `0` or `1` | `0` | Whether a page interrupts a call. |

## `type=translation` {#translation}

Each option is a voicemail folder name, and the value is the name that the phone displays.

```ini
[de_DE]
type=translation
INBOX=Eingang
Work=Arbeit
```
