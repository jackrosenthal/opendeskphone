---
title: "Manifest"
---

## `export_app!`

```rust
opendeskphone_app_sdk::export_app! {
    app: Weather,
    name: "weather",
    display_name: "Weather",
    description: "The weather where the phone is",
    kind: AppKind::Background,
    has_idle: true,
    icon: include_bytes!("weather.png"),
    glyph: opendeskphone_app_sdk::glyph!("weather-glyph.svg"),
    memory_kib: 8192,
}
```

`app` comes first and names the type that implements `App`.  The fields follow in any order.  The macro registers the type, embeds the manifest in the app file, and installs a panic hook that logs `panicked at <file>:<line>: <message>` at the error level before the phone stops the app.

## Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | `&str` | Required | Identifies the app, the app's [storage](storage.md), and the app's configuration (`<appconfig id="name">`).  1 to 64 characters: an ASCII letter or digit, then ASCII letters, digits, `_`, `.`, or `,`.  The phone rejects the name of a built-in app, and `dphone`. |
| `id` | `&str` | `""` | An identifier such as `"org.example.weather"`.  The phone stores and reports the value, and otherwise ignores the value. |
| `display_name` | `&str` | Required | The name that the phone shows, exactly as written.  No translations. |
| `description` | `&str` | `""` | One line, shown next to the name in the web interface. |
| `kind` | `AppKind` | `AppKind::Foreground` | See [App lifecycle](index.md#app-lifecycle). |
| `has_idle` | `bool` | `false` | Adds a window on the idle screen ([Idle window](ui.md#idle-window)).  The app then starts with the phone and keeps running off screen, whatever the `kind`. |
| `hidden` | `bool` | `false` | Hides the app from the Applications menu.  `runtime::show_app`, `kind`, and `has_idle` still start the app. |
| `icon` | `&[u8]` | `&[]` | PNG data, starting with the PNG signature, at most 256 KiB: `include_bytes!("weather.png")`. |
| `icon_for` | `&[(PhoneModel, &[u8])]` | `&[]` | Icons by model, each under the rules for `icon`, each model at most once: `&[(PhoneModel::D60, include_bytes!("color.png"))]`. |
| `phone_icon` | `Option<Icon>` | `None` | A [built-in icon](ui.md#built-in-icons), for a model with no PNG icon: `Some(Icon::AppCalendar)`. |
| `glyph` | `Glyph` | `Glyph::NONE` | The app in one color, from an SVG file: `opendeskphone_app_sdk::glyph!("weather-glyph.svg")`.  See [Glyph](#glyph). |
| `memory_kib` | `u32` | `0` | The linear memory limit in KiB.  `0` means 4096 (4 MiB).  Above 16384 (16 MiB) doesn't compile.  Data embedded with `include_bytes!` or `include_str!` counts toward the limit. |

## Icon selection

The Applications menu shows the first of these that exists:

1. The `icon_for` entry for the phone's model.
2. `icon`.
3. The built-in icon `phone_icon`.
4. The built-in app icon named by `name`.
5. The generic built-in app icon.

A PNG icon is scaled down, never up, to the size of the built-in app icons: 48 by 48 pixels on the color models.

## Glyph

Where the phone draws an app in one color, in the color of what the app is on, it shows the app's glyph instead of the icon.  The home buttons of the D80 are such a place: each button is lit while what it stands for is on, and dimmed while there is nothing to do, and an app's button has the app's glyph over the app's name.

The macro `glyph!` makes the glyph from an SVG file while the app compiles.  The macro is in the SDK's feature `glyph`:

```toml
[dependencies]
opendeskphone-app-sdk = { version = "=3.1.1", features = ["glyph"] }
```

The path is relative to the source file that has the macro, as for `include_bytes!`, and editing the SVG file recompiles the app.  The macro renders the SVG as tall as the phone's status icons, at each of their heights: 13, 15, 22, and 44 pixels.  Only the shape counts: the phone tints the glyph.  The app doesn't compile if the SVG file:

- has text or images,
- paints in more than one color: leave a hole where another color would be,
- paints with a gradient or a pattern,
- is more than twice as wide as it is tall, or
- renders empty.

The phone shows the first of these that exists:

1. The app's glyph at the height of the model's status icons, or else scaled down from the next taller height.  A glyph is never scaled up.
2. The built-in icon `phone_icon` if it is a one-color icon, else its [one-color twin](ui.md#built-in-icons).
3. The one-color twin of the built-in app icon named by `name`.
4. The built-in icon `apps`.

## Installation checks

The phone runs these checks in order and reports the first failure:

| Check | Message on failure |
| --- | --- |
| The file is at most 8 MiB. | The file is larger than any app may be. |
| The file is a WebAssembly component or module. | `not an app: an app is a WebAssembly component (a .wasm file made by wasm-tools component new)`.  A ZIP package of a JavaScript app for stock firmware is identified as such. |
| The file has a `opendeskphone-app` section. | `the WebAssembly file has no app manifest`. |
| The phone knows the manifest's kind. | `bad app kind N`. |
| The manifest's format number is 1. | `the app's manifest has format N, and this firmware knows format 1`. |
| `name` follows the rules under [Fields](#fields) and is not `opendeskphone`. | `bad app name: ...`. |
| Each icon is a PNG of at most 256 KiB, and each model in `icon_for` is a model of the interface's, once. | `bad app icon: ...`. |
| The glyph has at most 8 masks, each of its own height.  A mask is 1 to 128 pixels tall, at most twice as wide as it is tall, and has four bits of coverage for each of its pixels. | `bad app glyph: ...`. |
| The code sections total at most 1 MiB.  Data doesn't count. | The component holds more code than the phone compiles. |
| No built-in app has the same `name`. | `"name" is a built-in app`. |
| The flash has room for the file and the compiled code, with 2 MiB to spare. | Not enough space. |
| The component compiles, imports only interfaces of the firmware, and links against them. | `the app cannot be loaded: ...`, or `the app imports "<name>", which is not an interface of this firmware`. |

Installing an app with the `name` of an installed app replaces that app and keeps the app's storage.  A running instance of the replaced app receives `Event::Exit`, and the new version starts and runs off screen until the app is opened.  Removing an app deletes the app's storage on the phone, but not the app's keys on the PBX.

After a firmware update that changes the WebAssembly engine, the phone recompiles every installed app at startup.  Until an app is recompiled, opening the app fails with `the app is being compiled for this firmware; try again in a moment`.
