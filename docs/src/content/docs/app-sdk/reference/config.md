---
title: "config"
---

The phone's configuration can hold an element for the app, `<appconfig>`, whose `id` is the app's manifest `name`.  It has the form in which DPMA provisions the stock apps:

```xml
<appconfig id="weather">
  <account account_id="101" username="101" password="..." appserver="dasp://pbx/json" apiformat="json" />
  <settings units="metric" city="Huntsville" />
</appconfig>
```

The phone adds the credentials to the app's PBX requests; no function returns them.  Every running app that uses `config` receives [`Event::NewConfig`](app-events.md#newconfig) when the phone applies a new configuration.

## `app_config`

```rust
pub fn app_config() -> String
```

The app's `<appconfig>` element as XML text, or `""` if the configuration has none.  [`xml::parse`](helpers.md#xml) reads the text.  The text omits the credentials: the `password` and `secret` attributes of `<appconfig>` and of its children (`<account>` and `<settings>`).

## `app_setting`

```rust
pub fn app_setting(name: &str) -> Option<String>
```

One value of the app's configuration, by the first rule that applies:

1. `password` and `secret` return `None`.
2. `account_id` returns the `account_id` attribute of the app's `<account>`, if any.
3. The attribute of `<settings>` named `name`.
4. The attribute of `<appconfig>` named `name`.

`None` if nothing matches or the app has no `<appconfig>`.  In the example, `app_setting("units")` is `Some("metric")`, `app_setting("city")` is `Some("Huntsville")`, and `app_setting("account_id")` is `Some("101")`.

## `setting`

```rust
pub fn setting(id: PhoneSetting) -> Option<String>
```

One of the phone's settings, exactly as written in the configuration; `None` if the configuration doesn't set the setting.  `TimeFormat` and `DateFormat` are the active locale's (in `<locales>`, else `en_US`'s) unless the configuration sets its own.

| `PhoneSetting` | Setting in the phone's configuration |
| --- | --- |
| `Locale` | `locale` |
| `TimeZone` | `time_zone` |
| `TimeFormat` | `time_short_format` |
| `DateFormat` | `date_short_format` |
| `ContactsMaxSubscriptions` | `contacts_max_subscriptions` |
| `BlfContactGroup` | `blf_contact_group` |
| `DefaultRingtone` | `default_ringtone` |
| `NameFormat` | `name_format` |
| `Brightness` | `brightness` |

## `accounts`

```rust
pub fn accounts() -> Vec<AccountInfo>
```

The phone's SIP accounts ([`AccountInfo`](types.md#calls)).

## `model`

```rust
pub fn model() -> PhoneModel
```

```rust
pub enum PhoneModel { D40, D45, D50, D60, D62, D65, D70, D80 }
```

## `firmware_version`

```rust
pub fn firmware_version() -> String
```

The firmware version, with underscores as separators, e.g., `3_0_0`.

## `mac_address`

```rust
pub fn mac_address() -> String
```

The MAC address as 12 lowercase hexadecimal digits without separators, e.g., `000fd3a1b2c3`.

## `contact_lists`

```rust
pub fn contact_lists() -> Vec<(String, String)>
```

The provisioned contact list files, as pairs of file name and XML text.  Files that aren't valid UTF-8 are omitted.
