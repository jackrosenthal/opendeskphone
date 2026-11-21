---
title: "Your first app"
---

## Toolchain

Install the WebAssembly target, and `wasm-tools`, which turns the module that `cargo` builds into a component:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools
```

## Project setup

```sh
cargo new --lib freshpot
cd freshpot
```

Replace `Cargo.toml` with:

```toml
[package]
name = "freshpot"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
opendeskphone-app-sdk = "=3.0.0"

[profile.release]
opt-level = "s"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

- `crate-type = ["cdylib"]` makes `cargo` build a WebAssembly module.
- `opendeskphone-app-sdk = "=3.0.0"` is the SDK for firmware 3.0.0; the `=` makes Cargo use exactly that version.  The SDK version always matches the firmware version exactly.

## App code

Replace `src/lib.rs` with:

```rust
use opendeskphone_app_sdk::log_info;
use opendeskphone_app_sdk::prelude::*;

#[derive(Default)]
struct FreshPot;

impl App for FreshPot {
    fn start(&mut self) {
        let area = screen("Fresh Pot");
        let text = ui::create(WidgetKind::Text, ROOT, area);
        ui::set_text(text, "No pot has been brewed yet.");
        log_info("the first screen is built");
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Key(Key::Cancel) => runtime::exit(),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: FreshPot,
    name: "freshpot",
    display_name: "Fresh Pot",
    description: "When the office coffee was brewed",
}
```

- `FreshPot` holds the app's state.  At startup, the phone creates the value with `Default::default()`.
- The phone calls `start` once, then `handle` for each event.
- [`screen`](../reference/helpers.md#screen) clears the window, draws a title, claims the cancel key (✕), and returns the rectangle below the title.  [`ui::create`](../reference/ui.md#create) makes a `Text` widget in that rectangle, a child of the window (`ROOT`).
- A cancel key press arrives as `Event::Key(Key::Cancel)`.  [`runtime::exit`](../reference/runtime.md#exit) stops the app and removes the window.  On the D80, which has a touchscreen and no keys, the back button at the top left of the window is the cancel key, and the softkeys of the later parts are buttons; the apps of this tutorial work there as written (see [Touch](../reference/ui.md#touch)).
- `export_app!` registers the type and embeds the manifest in the app file.  Installing an app replaces the installed app with the same `name`, and the phone rejects the `name` of a built-in app.  The Applications menu shows `display_name`, and the web interface shows `description`.  The [manifest reference](../reference/manifest.md) lists the other fields.

## Build

In the crate directory:

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new \
    target/wasm32-unknown-unknown/release/freshpot.wasm -o freshpot.wasm
```

`freshpot.wasm` is the whole app, including the manifest and embedded data.

## Installation

1. Browse to the phone's IP address and sign in.
2. On the **Apps** page, click **Install App** and choose `freshpot.wasm`.
3. Wait a few seconds while the phone compiles the app.

A newer build installs the same way.  If the app is running, the phone stops the running copy and starts the new build off screen; open the app to see the new build.

## First run

Open Fresh Pot from the Applications menu (**Applications** in the main menu, or the **Apps** softkey on the idle screen), or click **Open** on the **Apps** page.  The **Screen** page mirrors the display and has the phone's keys.

<figure class="screenshot">

![The first screen of Fresh Pot: a title and one line of text](../../assets/app-sdk/first-screen.png){ width="480" }

</figure>

Press the cancel key to exit.

## Logs

On the **Logs** page, type `freshpot` in **Filter**.  The page shows the app's `log_info` line and the phone's lines about the app:

```text
INFO  opendeskphone_apps::manager app freshpot: installed
INFO  app freshpot: the first screen is built
INFO  opendeskphone_apps::manager app freshpot: started
INFO  opendeskphone_apps::manager app freshpot: stopped
```

After a panic, the phone stops the app, and the log shows the panic message with file and line:

```text
ERROR app freshpot: panicked at src/lib.rs:81: the pot is empty
```

A second error line, from the phone, starts with `app freshpot: stopped after a fault` and includes a WebAssembly backtrace.
