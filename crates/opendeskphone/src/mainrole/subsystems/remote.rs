//! Diagnostics that drive the phone as a user would: keys, touches, the
//! hook, calls, apps, and a screenshot of what is on the display.  For SSH sessions and
//! for testing without touching the phone.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use opendeskphone_hal::{PixelFormat, Screen};
use opendeskphone_state::{Command, Key, Store, TouchPhase};

use super::ui::ScreenTap;
use crate::diag::Output;
use crate::mainrole::Context;

async fn send(store: &Store, cmd: Command) -> Result<String, String> {
    match tokio::time::timeout(Duration::from_secs(15), store.send(cmd)).await {
        Ok(Ok(reply)) => Ok(format!("{reply:?}\n")),
        Ok(Err(e)) => Err(format!("{e:?}")),
        Err(_) => Err("no answer within 15 s".into()),
    }
}

/// A frame as a binary PGM (gray) or PPM (color) picture.
fn netpbm(frame: &opendeskphone_hal::Frame) -> Vec<u8> {
    let (w, h) = (frame.info.width as usize, frame.info.height as usize);
    let mut out = Vec::with_capacity(w * h * 3 + 32);
    match frame.info.format {
        PixelFormat::R8 => {
            out.extend_from_slice(format!("P5\n{w} {h}\n255\n").as_bytes());
            for y in 0..h {
                out.extend_from_slice(&frame.pixels[y * frame.info.stride..y * frame.info.stride + w]);
            }
        }
        PixelFormat::Xrgb8888 => {
            out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
            for y in 0..h {
                for px in frame.pixels[y * frame.info.stride..y * frame.info.stride + w * 4].as_chunks::<4>().0 {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
        }
    }
    out
}

/// Press keys as if on the keypad
#[derive(Debug, Parser)]
#[command(name = "key", after_help = KEY_NAMES)]
pub struct KeyCommand {
    /// The keys, pressed and released one after the other (e.g., 1 2 3 sk1)
    #[arg(required = true, value_name = "KEY", value_parser = parse_key)]
    keys: Vec<Key>,
}

const KEY_NAMES: &str = "\
Keys:
  0-9 * # up down left right select cancel sk1..sk4 line1..line6 vup vdn
  mute spkr hdst hold xfer redial msgs conf info blfleft blfright status
  dir apps menu";

fn parse_key(name: &str) -> Result<Key, String> {
    Key::from_name(name).ok_or_else(|| "not a key ('dphone key --help' lists them)".to_string())
}

/// Touch the screen as if with a finger (D80)
///
/// A tap at X, Y, or with --to, a drag from there.  X and Y are pixels of
/// the main screen, from its top-left corner.
#[derive(Debug, Parser)]
#[command(name = "touch", after_help = TOUCH_EXAMPLES)]
pub struct TouchCommand {
    /// Pixels from the left edge
    x: u16,
    /// Pixels from the top edge
    y: u16,
    /// Drag the finger to here, and lift it there
    #[arg(long, num_args = 2, value_names = ["X", "Y"])]
    to: Option<Vec<u16>>,
    /// The number of moves a drag takes
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=1000))]
    steps: u16,
}

const TOUCH_EXAMPLES: &str = "\
Examples:
  dphone touch 400 640                a tap in the middle of the D80's screen
  dphone touch 400 1000 --to 400 300  a drag upward: a list moves on to its later rows";

impl TouchCommand {
    /// The events of the touch, in order.
    fn events(&self) -> Vec<(TouchPhase, u16, u16)> {
        let (x, y) = (self.x, self.y);
        let mut events = vec![(TouchPhase::Down, x, y)];
        match self.to.as_deref() {
            Some(&[x1, y1]) => {
                // As `SimHandle::touch_drag` moves.
                let n = self.steps.max(1) as i32;
                let at = |a: u16, b: u16, i: i32| (a as i32 + (b as i32 - a as i32) * i / n) as u16;
                events.extend((1..=n).map(|i| (TouchPhase::Move, at(x, x1, i), at(y, y1, i))));
                events.push((TouchPhase::Up, x1, y1));
            }
            _ => events.push((TouchPhase::Up, x, y)),
        }
        events
    }
}

/// Lift or replace the handset
#[derive(Debug, Parser)]
#[command(name = "hook")]
pub struct HookCommand {
    state: HookState,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
enum HookState {
    /// Off the hook: the handset is lifted
    Off,
    /// On the hook: the handset is in its cradle
    On,
}

/// Make, answer, and end calls
///
/// Without a subcommand: list.
#[derive(Debug, Parser)]
#[command(name = "call")]
pub struct CallCommand {
    #[command(subcommand)]
    action: Option<CallAction>,
}

#[derive(Debug, Subcommand)]
enum CallAction {
    /// Show the calls
    List,
    /// Call a number
    Dial { number: String },
    /// Answer the first call
    Answer,
    /// End every call
    Hangup,
}

/// Start and close apps
///
/// Without a subcommand: list.
#[derive(Debug, Parser)]
#[command(name = "app")]
pub struct AppCommand {
    #[command(subcommand)]
    action: Option<AppAction>,
}

#[derive(Debug, Subcommand)]
enum AppAction {
    /// Show the installed apps
    List,
    /// Start an app, or bring it to the front
    Launch { name: String },
    /// Close an app
    Close { name: String },
}

/// Write what is on a display to a file (PPM, or PGM for a gray display)
#[derive(Debug, Parser)]
#[command(name = "screenshot")]
pub struct ScreenshotCommand {
    /// The file to write; a relative name is relative to the main role's
    /// working directory, not the caller's
    file: PathBuf,
    /// Which display
    #[arg(long, default_value = "main")]
    screen: ScreenArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScreenArg {
    Main,
    Side,
}

pub fn register(ctx: &Context, tap: Option<Arc<ScreenTap>>) {
    let store = ctx.store.clone();
    ctx.diag.register(move |args: KeyCommand, out: Output| {
        let store = store.clone();
        async move {
            for key in args.keys {
                for down in [true, false] {
                    if let Err(e) = send(&store, Command::InjectKey { key, down }).await {
                        out.err(&format!("dphone key: {e}\n")).await;
                        return 1;
                    }
                    tokio::time::sleep(Duration::from_millis(60)).await;
                }
            }
            0
        }
    });

    let store = ctx.store.clone();
    ctx.diag.register(move |args: TouchCommand, out: Output| {
        let store = store.clone();
        async move {
            let model = store.model();
            if !model.is_touch() {
                out.err(&format!("dphone touch: the {model} has no touchscreen\n")).await;
                return 1;
            }
            let size = model.main_screen();
            let events = args.events();
            if let Some((_, x, y)) = events.iter().find(|(_, x, y)| *x >= size.width || *y >= size.height) {
                out.err(&format!("dphone touch: ({x}, {y}) is not on the {}x{} screen\n", size.width, size.height)).await;
                return 1;
            }
            for (phase, x, y) in events {
                if let Err(e) = send(&store, Command::InjectTouch { phase, x, y }).await {
                    out.err(&format!("dphone touch: {e}\n")).await;
                    return 1;
                }
                // About a finger's pace: a tap is lifted well before a long
                // press, and a drag moves every 20 ms.
                let pause = if phase == TouchPhase::Down && args.to.is_none() { 60 } else { 20 };
                tokio::time::sleep(Duration::from_millis(pause)).await;
            }
            0
        }
    });

    let store = ctx.store.clone();
    ctx.diag.register(move |args: HookCommand, out: Output| {
        let store = store.clone();
        async move {
            match send(&store, Command::InjectHook { off_hook: args.state == HookState::Off }).await {
                Ok(_) => 0,
                Err(e) => {
                    out.err(&format!("dphone hook: {e}\n")).await;
                    1
                }
            }
        }
    });

    let store = ctx.store.clone();
    ctx.diag.register(move |args: CallCommand, out: Output| {
        let store = store.clone();
        async move {
            let calls = store.get::<opendeskphone_state::Calls>();
            let result = match args.action.unwrap_or(CallAction::List) {
                CallAction::Dial { number } => send(&store, Command::Dial { target: number, account: None, path: None }).await,
                CallAction::Answer => match calls.calls.first() {
                    Some(c) => send(&store, Command::Answer { call: c.id, path: None }).await,
                    None => Err("no call".into()),
                },
                CallAction::Hangup => {
                    let mut last = Err("no call".to_string());
                    for c in &calls.calls {
                        last = send(&store, Command::Hangup { call: c.id }).await;
                    }
                    last
                }
                CallAction::List => Ok(format!("{calls:#?}\n")),
            };
            match result {
                Ok(text) => {
                    out.out(&text).await;
                    0
                }
                Err(e) => {
                    out.err(&format!("dphone call: {e}\n")).await;
                    1
                }
            }
        }
    });

    let store = ctx.store.clone();
    ctx.diag.register(move |args: AppCommand, out: Output| {
        let store = store.clone();
        async move {
            let result = match args.action.unwrap_or(AppAction::List) {
                AppAction::Launch { name } => send(&store, Command::LaunchApp { name }).await,
                AppAction::Close { name } => send(&store, Command::CloseApp { name }).await,
                AppAction::List => Ok(store
                    .get::<opendeskphone_state::Apps>()
                    .installed
                    .iter()
                    .map(|a| format!("{:<12} {}{}\n", a.name, a.display_name, if a.running { " (running)" } else { "" }))
                    .collect()),
            };
            match result {
                Ok(text) => {
                    out.out(&text).await;
                    0
                }
                Err(e) => {
                    out.err(&format!("dphone app: {e}\n")).await;
                    1
                }
            }
        }
    });

    ctx.diag.register(move |args: ScreenshotCommand, out: Output| {
        let tap = tap.clone();
        async move {
            let screen = match args.screen {
                ScreenArg::Main => Screen::Main,
                ScreenArg::Side => Screen::Side,
            };
            // (The copy is taken by the UI's thread, which may take a
            // moment.)
            let frame = tokio::task::spawn_blocking(move || tap.as_ref().and_then(|t| t.capture(screen))).await.ok().flatten();
            let Some(frame) = frame else {
                out.err("dphone screenshot: nothing has been drawn on that screen\n").await;
                return 1;
            };
            let file = args.file.display();
            match std::fs::write(&args.file, netpbm(&frame)) {
                Ok(()) => {
                    out.out(&format!("{}x{} written to {file}\n", frame.info.width, frame.info.height)).await;
                    0
                }
                Err(e) => {
                    out.err(&format!("dphone screenshot: {file}: {e}\n")).await;
                    1
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use TouchPhase::*;

    fn touch(words: &[&str]) -> TouchCommand {
        TouchCommand::try_parse_from(std::iter::once("touch").chain(words.iter().copied())).unwrap()
    }

    #[test]
    fn a_touch_is_a_tap_or_a_drag_in_steps() {
        assert_eq!(touch(&["400", "640"]).events(), [(Down, 400, 640), (Up, 400, 640)]);
        assert_eq!(
            touch(&["10", "1000", "--to", "20", "400", "--steps", "3"]).events(),
            [(Down, 10, 1000), (Move, 13, 800), (Move, 16, 600), (Move, 20, 400), (Up, 20, 400)]
        );
        // Ten moves unless told.
        assert_eq!(touch(&["0", "0", "--to", "0", "100"]).events().len(), 12);
    }
}
