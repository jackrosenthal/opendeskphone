//! The firmware's idle screen for each model, drawn by the phone's own UI
//! on the HAL simulator, as the UI's snapshot test (`crates/opendeskphone-ui/tests/snapshots.rs`)
//! draws the screen: the same accounts, keys, and time.

use crate::panel::ModelId;
use opendeskphone_hal::{Frame, Hal, Screen};
use opendeskphone_state::*;
use opendeskphone_ui::{Ui, UiConfig};
use std::net::Ipv4Addr;

const MONO: u64 = 5_000_000;
/// 2026-11-13 21:58:07 UTC.
const UNIX: u64 = 1_794_607_087;

/// An RGB picture, row by row.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The idle screen of the main display and, on the D70, of the side display.
pub struct Screens {
    pub main: Picture,
    pub side: Option<Picture>,
}

/// The color models show `theme`'s wallpaper and palette; the gray models
/// have no themes.
pub fn idle(model: ModelId, theme: &str) -> Screens {
    let model = match model {
        ModelId::D40 => Model::D40,
        ModelId::D50 => Model::D50,
        ModelId::D60 => Model::D60,
        ModelId::D65 => Model::D65,
        ModelId::D70 => Model::D70,
        ModelId::D80 => Model::D80,
    };
    let mut hal = Hal::sim(model);
    let sim = hal.sim.clone().expect("the simulator");
    let store = Store::new(model);
    let cfg = UiConfig {
        utc_offset_secs: -7 * 3600,
        ringtone: "Flutey Phone".into(),
        ringtones: vec!["Flutey Phone".into(), "Acheron".into(), "Andromeda".into()],
        theme: theme.into(),
        ..Default::default()
    };
    let mut ui = Ui::from_hal(&mut hal, store.clone(), cfg);
    ui.set_fixed_time(Some((MONO, UNIX)));
    running(&store, model);
    ui.pump();
    Screens {
        main: picture(&sim.frame(Screen::Main).expect("the main display's frame")),
        side: sim.frame(Screen::Side).map(|f| picture(&f)),
    }
}

fn picture(frame: &Frame) -> Picture {
    let (w, h) = (frame.info.width as usize, frame.info.height as usize);
    let bpp = frame.info.format.bytes_per_pixel();
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let i = y * frame.info.stride + x * bpp;
            if bpp == 4 {
                rgb.extend_from_slice(&[frame.pixels[i + 2], frame.pixels[i + 1], frame.pixels[i]]);
            } else {
                rgb.extend_from_slice(&[frame.pixels[i]; 3]);
            }
        }
    }
    Picture { width: w as u32, height: h as u32, rgb }
}

/// A phone that is up, registered, and idle: `Rig::running` of the
/// snapshot test.
fn running(store: &Store, model: Model) {
    store.update::<System, _>(|s| {
        s.phase = Phase::Running;
        s.time_synced = true;
        s.mac = Some([0x00, 0x0f, 0xd3, 0x12, 0x34, 0x56]);
        s.hostname = "dphone-123456".into();
        s.mem_total_kib = 113 * 1024;
    });
    store.update::<Network, _>(|n| {
        n.phase = NetPhase::Up;
        n.lan = PortLink { up: true, speed: 100, full_duplex: true };
        n.ipv4 = Ipv4State {
            source: AddrSource::Dhcp,
            address: Some(Ipv4Addr::new(10, 10, 15, 87)),
            prefix_len: 24,
            gateway: Some(Ipv4Addr::new(10, 10, 15, 1)),
            ..Default::default()
        };
        n.dns = vec![Ipv4Addr::new(10, 10, 15, 1).into()];
    });
    store.update::<Accounts, _>(|a| {
        for i in 0..model.max_accounts().min(2) {
            let mut acct = Account::new(AccountId(i));
            acct.label = format!("10{i}");
            acct.user = format!("10{i}");
            acct.display_name = if i == 0 { "Jordan Example".into() } else { "Front Desk".into() };
            acct.server = "pbx.example.invalid".into();
            acct.registration = if i == 0 {
                Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: MONO + 60_000 }
            } else {
                Registration::Failed { code: 403, reason: "Forbidden".into(), retry_mono_ms: MONO + 30_000 }
            };
            a.accounts.push(acct);
        }
        a.default_account = Some(AccountId(0));
    });
    store.update::<Presence, _>(|p| {
        for (i, (name, state)) in
            [("Alice", BlfState::Idle), ("Bob", BlfState::InUse), ("Carol", BlfState::Ringing), ("Dave", BlfState::Unavailable)]
                .into_iter()
                .enumerate()
        {
            p.entries.push(BlfEntry {
                id: format!("blf{i}"),
                label: name.into(),
                number: format!("20{i}"),
                state,
                status_text: if i == 0 { "Available".into() } else { String::new() },
                ..Default::default()
            });
        }
    });
    store.update::<Keys, _>(|k| {
        for i in 0..model.line_keys() {
            let binding = match i {
                0 | 1 => LineKeyBinding::Account(AccountId(i)),
                2 | 3 => LineKeyBinding::Blf(format!("blf{}", i - 2)),
                _ => LineKeyBinding::Unused,
            };
            k.line_keys.push(LineKey { index: i, binding, label: String::new(), lamp: LampState::OFF, calls: vec![] });
        }
        for i in 0..model.blf_keys().min(6) {
            k.rapid_dial.push(RapidDialKey {
                index: i,
                page: 0,
                entry: format!("blf{}", i % 4),
                label: ["Alice", "Bob", "Carol", "Dave", "Sales Queue", "Parking 701"][i as usize].into(),
                number: format!("20{i}"),
                lamp: if i == 1 { LampState::solid(LampColor::Red) } else { LampState::solid(LampColor::Green) },
            });
        }
        k.rapid_pages = 3;
        k.message_lamp = LampState::blinking(LampColor::Red, Blink::Slow);
    });
    store.update::<Mwi, _>(|m| {
        m.accounts.push(MwiAccount { account: AccountId(0), waiting: true, new: 3, old: 1, urgent_new: 0, urgent_old: 0 })
    });
    store.update::<Apps, _>(|a| {
        for (name, display) in [("contacts", "Contacts"), ("voicemail", "Voicemail"), ("status", "Status"), ("parking", "Parking")] {
            a.installed.push(AppInfo {
                name: name.into(),
                id: format!("org.opendeskphone.{name}"),
                display_name: display.into(),
                builtin: true,
                icon: format!("app-{name}"),
                ..Default::default()
            });
        }
    });
    store.update::<Audio, _>(|a| a.dsp = DspStatus::Running);
}
