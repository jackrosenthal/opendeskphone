//! A workstation stand-in for the phone, for developing and testing the
//! web interface: the real web server over a state store filled with
//! made-up state, the HAL simulator's display, and stub command handlers
//! that behave plausibly (a dialed call rings and connects; a firmware
//! install counts through its phases).
//!
//! ```text
//! cargo run -p opendeskphone-web --example sim -- --model d60 --port 8480
//! ```
//!
//! Then open <http://127.0.0.1:8480/> (user `admin`, password `789`).

use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_hal::{Display, Hal, InputEvent, PixelFormat, Rect, Screen};
use opendeskphone_state::{self as st, Command, CommandError, Domain, Key, Model, Reply, Store};
use opendeskphone_web::{
    FileFirmwareSink, FirmwareInfo, MirrorFormat, MirrorFrame, MirrorScreen, ScreenSource, WebOptions, WebServer,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct SimScreen(opendeskphone_hal::sim::SimHandle);

impl ScreenSource for SimScreen {
    fn generation(&self) -> u64 {
        self.0.frame_generation()
    }
    fn capture(&self, screen: MirrorScreen) -> Option<MirrorFrame> {
        let f = self.0.frame(match screen {
            MirrorScreen::Main => Screen::Main,
            MirrorScreen::Side => Screen::Side,
        })?;
        Some(MirrorFrame {
            width: f.info.width,
            height: f.info.height,
            stride: f.info.stride,
            format: match f.info.format {
                PixelFormat::Xrgb8888 => MirrorFormat::Xrgb8888,
                PixelFormat::R8 => MirrorFormat::Gray8,
            },
            pixels: f.pixels,
        })
    }
}

// A 3x5 font, enough to show what was typed.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        '*' => [5, 2, 7, 2, 5],
        '#' => [5, 7, 5, 7, 5],
        '-' => [0, 0, 7, 0, 0],
        _ => [0, 0, 0, 0, 0],
    }
}

struct Painter {
    display: Box<dyn Display>,
}

impl Painter {
    fn fill(&mut self, screen: Screen, r: Rect, rgb: (u8, u8, u8)) {
        let Some(info) = self.display.info(screen) else { return };
        let r = r.clip(info.width, info.height);
        let Ok(buf) = self.display.frame(screen) else { return };
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                let o = y as usize * info.stride + x as usize * info.format.bytes_per_pixel();
                match info.format {
                    PixelFormat::Xrgb8888 => buf[o..o + 4].copy_from_slice(&[rgb.2, rgb.1, rgb.0, 0]),
                    PixelFormat::R8 => {
                        buf[o] = ((rgb.0 as u16 * 3 + rgb.1 as u16 * 6 + rgb.2 as u16) / 10) as u8;
                    }
                }
            }
        }
    }

    fn text(&mut self, screen: Screen, x: u16, y: u16, scale: u16, s: &str, rgb: (u8, u8, u8)) {
        let mut cx = x;
        for c in s.chars() {
            for (row, bits) in glyph(c).iter().enumerate() {
                for col in 0..3u16 {
                    if bits & (4 >> col) != 0 {
                        self.fill(screen, Rect::new(cx + col * scale, y + row as u16 * scale, scale, scale), rgb);
                    }
                }
            }
            cx += 4 * scale;
        }
    }

    /// The whole picture: a title bar, the typed digits, the softkey bar,
    /// and a block per softkey that lights while held.
    fn paint(&mut self, model: Model, dial: &str, held: Option<Key>, off_hook: bool) {
        let size = model.main_screen();
        let bar = model.softkey_bar_height();
        let scale = if model.is_color() { 4 } else { 2 };
        self.fill(Screen::Main, Rect::new(0, 0, size.width, size.height), (245, 247, 250));
        self.fill(Screen::Main, Rect::new(0, 0, size.width, bar), (13, 148, 136));
        if off_hook {
            self.fill(Screen::Main, Rect::new(size.width - bar, 2, bar - 4, bar - 4), (250, 204, 21));
        }
        self.text(Screen::Main, 8, bar + 12, scale, dial, (15, 23, 42));
        self.fill(Screen::Main, Rect::new(0, size.height - bar, size.width, bar), (30, 41, 59));
        for i in 0..4u16 {
            let lit = held == Some(Key::Soft(i as u8 + 1));
            let w = size.width / 4;
            self.fill(
                Screen::Main,
                Rect::new(i * w + 3, size.height - bar + 3, w - 6, bar - 6),
                if lit { (45, 212, 191) } else { (71, 85, 105) },
            );
        }
        let _ = self.display.flush(Screen::Main, &[]);
        if let Some(side) = model.side_screen() {
            self.fill(Screen::Side, Rect::new(0, 0, side.width, side.height), (255, 255, 255));
            for i in 0..10u16 {
                self.fill(Screen::Side, Rect::new(4, 6 + i * 35, side.width - 8, 2), (60, 60, 60));
                self.text(Screen::Side, 8, 14 + i * 35, 2, &format!("{}", 100 + i), (0, 0, 0));
            }
            let _ = self.display.flush(Screen::Side, &[]);
        }
    }
}

fn fabricate(store: &Store, model: Model) {
    store.update::<st::System, _>(|s| {
        s.phase = st::Phase::Running;
        s.mac = Some([0x00, 0x0f, 0xd3, 0x12, 0x34, 0x56]);
        s.hostname = format!("{}-123456", model.name().to_lowercase());
        s.time_synced = true;
        s.timezone = "America/Los_Angeles".into();
        s.last_reboot_reason = "web interface".into();
        s.simulated = true;
        s.boot_rev = Some(3);
        s.mem_total_kib = model.ram_mib() as u32 * 1024 - 15_000;
    });
    store.update::<st::Network, _>(|n| {
        n.phase = st::NetPhase::Up;
        n.lan = st::PortLink { up: true, speed: if model.gigabit() { 1000 } else { 100 }, full_duplex: true };
        n.pc = st::PortLink { up: false, speed: 0, full_duplex: false };
        n.pc_port_enabled = true;
        n.vlan = st::VlanState { source: st::VlanSource::Lldp, voice_id: Some(20), voice_priority: 5, dscp: 46, pc_id: None };
        n.ipv4 = st::Ipv4State {
            source: st::AddrSource::Dhcp,
            address: Some("10.10.20.57".parse().unwrap()),
            prefix_len: 24,
            gateway: Some("10.10.20.1".parse().unwrap()),
            lease_secs: 86_400,
            lease_obtained_mono_ms: st::clock::mono_ms(),
            dhcp_server: Some("10.10.20.1".parse().unwrap()),
        };
        n.dns = vec!["10.10.20.1".parse().unwrap(), "1.1.1.1".parse().unwrap()];
        n.domain = "example.test".into();
        n.dhcp_tftp_server = "sip:pbx.example.test".into();
        n.ntp_servers = vec!["pool.ntp.org".into()];
        n.ntp_synced_mono_ms = Some(st::clock::mono_ms());
        n.lldp = Some(st::LldpNeighbor {
            chassis_id: "aa:bb:cc:00:11:22".into(),
            port_id: "ge-0/0/14".into(),
            system_name: "access-sw-2".into(),
            port_description: "Desk 14".into(),
        });
        n.profile = "office".into();
    });
    store.update::<st::Provisioning, _>(|p| {
        p.phase = st::ProvisionPhase::Provisioned;
        let server = st::DiscoveredServer {
            source: st::DiscoverySource::DhcpOption66,
            name: "pbx.example.test".into(),
            address: "pbx.example.test:5060".into(),
        };
        p.discovered = vec![
            server.clone(),
            st::DiscoveredServer { source: st::DiscoverySource::Mdns, name: "Lab PBX".into(), address: "10.10.20.9:5060".into() },
        ];
        p.server = Some(server);
        p.server_user_agent = "Asterisk PBX 22.4.0".into();
        p.assigned_user = "Alex Reed (100)".into();
        p.attached = true;
        p.config_md5 = "9e107d9d372bb6826bd81d3542a419d6".into();
        p.last_success_unix = Some(st::clock::unix_secs() - 3600);
    });
    store.update::<st::Accounts, _>(|a| {
        let mut one = st::Account::new(st::AccountId(0));
        one.label = "100".into();
        one.display_name = "Alex Reed".into();
        one.user = "100".into();
        one.server = "pbx.example.test".into();
        one.dpma = true;
        one.registration =
            st::Registration::Registered { host: st::ActiveHost::Primary, expires_mono_ms: st::clock::mono_ms() + 240_000 };
        let mut two = st::Account::new(st::AccountId(1));
        two.label = "Support".into();
        two.user = "8100".into();
        two.server = "sip.backup.test".into();
        two.transport = st::Transport::Tls;
        two.port = 5061;
        two.registration = st::Registration::Failed {
            code: 403,
            reason: "Forbidden".into(),
            retry_mono_ms: st::clock::mono_ms() + 20_000,
        };
        a.accounts = vec![one, two];
        a.default_account = Some(st::AccountId(0));
    });
    store.update::<st::Keys, _>(|k| {
        k.line_keys = (0..model.lines())
            .map(|i| st::LineKey {
                index: i,
                binding: if i < 2 { st::LineKeyBinding::Account(st::AccountId(i)) } else { st::LineKeyBinding::Unused },
                label: ["100", "Support"].get(i as usize).unwrap_or(&"").to_string(),
                lamp: match i {
                    0 => st::LampState::solid(st::LampColor::Green),
                    1 => st::LampState::blinking(st::LampColor::Red, st::Blink::Slow),
                    _ => st::LampState::OFF,
                },
                calls: Vec::new(),
            })
            .collect();
        k.rapid_dial = (0..model.blf_keys())
            .map(|i| st::RapidDialKey {
                index: i,
                page: 0,
                entry: format!("c{i}"),
                label: format!("Ext {}", 101 + i as u16),
                number: format!("{}", 101 + i as u16),
                lamp: if i == 2 { st::LampState::solid(st::LampColor::Red) } else { st::LampState::OFF },
            })
            .collect();
        k.message_lamp = st::LampState::blinking(st::LampColor::Red, st::Blink::Wink);
    });
    store.update::<st::Mwi, _>(|m| {
        m.accounts =
            vec![st::MwiAccount { account: st::AccountId(0), waiting: true, new: 2, old: 5, urgent_new: 0, urgent_old: 0 }];
    });
    store.update::<st::CallLog, _>(|l| {
        let now = st::clock::unix_secs();
        for (i, (kind, name, number, dur)) in [
            (st::CallLogKind::Placed, "Morgan Li", "104", 312),
            (st::CallLogKind::Received, "Front Desk", "100", 45),
            (st::CallLogKind::Missed, "", "5551230188", 0),
            (st::CallLogKind::Missed, "Sam Ortiz", "117", 0),
        ]
        .into_iter()
        .enumerate()
        {
            l.push(st::CallLogEntry {
                id: 0,
                kind,
                party: st::Party { name: name.into(), number: number.into(), uri: String::new() },
                account: st::AccountId(0),
                start_unix: now - 7200 + i as u64 * 900,
                duration_secs: dur,
            });
        }
    });
    store.update::<st::Audio, _>(|a| a.dsp = st::DspStatus::Running);
    store.update::<st::Update, _>(|u| u.running_bank = Some(st::Bank::B));
    store.update::<st::Apps, _>(|a| {
        let app = |name: &str, display: &str, desc: &str| st::AppInfo {
            name: name.into(),
            id: name.into(),
            display_name: display.into(),
            description: desc.into(),
            kind: st::AppKind::Foreground,
            builtin: true,
            ..Default::default()
        };
        a.installed = vec![
            app("contacts", "Contacts", "Phone book and presence"),
            app("voicemail", "Voicemail", "Visual voicemail"),
            app("status", "Status", "Set your presence"),
            app("parking", "Parking", "Parked calls"),
            app("queues", "Queues", "Call queue membership"),
            app("hotdesking", "Hot Desking", "Log in to this phone"),
            app("calendar", "Calendar", "Month calendar"),
            app("alarms", "Alarms", "Set the alarm clock"),
            app("boxpusher", "Box Pusher", "Push every box onto a target"),
            app("desksparrow", "Desk Sparrow", "A one-key flying game"),
            app("lightup", "Light Up", "Light every cell of the grid"),
        ];
    });
    store.update::<st::Ui, _>(|u| {
        u.screen = "idle".into();
        u.backlight_on = true;
        u.backlight_level = 20;
    });
    for s in st::Subsystem::ALL {
        store.set_health(
            s,
            match s {
                st::Subsystem::Media => st::HealthStatus::Degraded("simulated audio".into()),
                st::Subsystem::Web => st::HealthStatus::Starting,
                _ => st::HealthStatus::Ok,
            },
        );
    }
    store.update::<st::Health, _>(|h| h.healthy = true);

    store.update::<st::Alarms, _>(|a| {
        let alarm = |id, hour, minute, days, tone: &str, enabled| st::Alarm { id, hour, minute, days, tone: tone.into(), enabled };
        a.alarms = vec![
            alarm(1, 6, 30, 0b0111110, "Osmium", true),
            alarm(2, 8, 0, 0b0000010, "Beep-Beep-Beep", true),
            alarm(3, 9, 30, 0b1000001, "Buzzer", false),
            alarm(4, 14, 45, 0, "Beep-Beep-Beep", true),
        ];
        // The phone's alarm tones (`alarmtones::ALL` in the main role).
        a.tones = [
            "Beep-Beep-Beep", "Argon", "Argon JP", "Awaken", "Barium", "BeeBeep", "Bounce", "Buzzer", "Carbon", "Carbon JP",
            "Cesium", "Copernicium", "Curium", "Drip", "Fermium", "Gallop", "Hassium", "Helium", "Helium JP", "Krypton",
            "Neon", "Neptunium", "Nobelium", "Nudge", "Orbit", "Osmium", "Osmium Classic", "Oxygen", "Piezo", "Platinum",
            "Plutonium", "Promethium", "Rise", "Rooster 1", "Rooster 2", "Scandium", "Sway", "Timer", "Wag",
        ]
        .map(String::from)
        .to_vec();
        a.icon = true;
        a.snooze_secs = 600;
        a.next = Some((4, st::clock::unix_secs() + 2 * 3600));
    });

    store.update::<st::Paging, _>(|p| {
        let group = |id: &str, name: &str, last: u8| st::PageGroup {
            id: id.into(),
            name: name.into(),
            addr: format!("239.1.1.{last}"),
            port: 5004,
        };
        p.listeners = vec![group("1", "All phones", 1), group("2", "Warehouse", 2)];
        p.broadcasts = vec![group("1", "All phones", 1), group("3", "Front office", 3)];
    });
    if model.expansion_modules() {
        store.update::<st::Expansion, _>(|e| {
            e.enabled = true;
            e.modules = vec![
                st::ExpansionModule { node: 0, addr: "192.168.218.2".into(), version: "EXP150 1_2_0".into(), page: 0 },
                st::ExpansionModule { node: 1, addr: "192.168.218.3".into(), version: "EXP150 1_2_0".into(), page: 1 },
            ];
        });
    }
    if model.bluetooth() {
        store.update::<st::Bluetooth, _>(|b| {
            b.present = true;
            b.powered = true;
            b.address = "00:0F:D3:65:00:01".into();
            b.paired = vec![st::BluetoothDevice { addr: "00:1B:DC:0F:A1:02".into(), name: "Voyager 5200".into(), rssi: 0 }];
            b.connected = "00:1B:DC:0F:A1:02".into();
        });
    }
}

/// Runs a handler thread for one command domain.
fn handle(store: &Store, domain: Domain, mut f: impl FnMut(&Store, &Command) -> Result<Reply, CommandError> + Send + 'static) {
    let rx = store.handle(domain);
    let store = store.clone();
    std::thread::spawn(move || {
        while let Some(env) = rx.recv() {
            log::debug!("command: {:?}", env.command);
            let r = f(&store, &env.command);
            env.reply(r);
        }
    });
}

fn unsupported(c: &Command) -> Result<Reply, CommandError> {
    Err(CommandError::Invalid(format!("the simulator does not implement {c:?}")))
}

fn spawn_handlers(store: &Store, hal: Hal, config: Arc<Mutex<ConfigStore>>) {
    let model = hal.model;
    let sim = hal.sim.clone().expect("sim backend");
    let painter = Arc::new(Mutex::new(Painter { display: hal.display }));
    painter.lock().unwrap().paint(model, "", None, false);

    // Ui: keys go to the simulated keypad and onto the picture.
    let p = painter.clone();
    let sim2 = sim.clone();
    let mut held: Option<Key> = None;
    handle(store, Domain::Ui, move |store, c| match c {
        Command::InjectKey { key, down } => {
            sim2.inject(InputEvent::Key { key: *key, down: *down });
            held = down.then_some(*key);
            if *down {
                store.update::<st::Ui, _>(|u| match key {
                    Key::Cancel => {
                        u.dial_buffer.pop();
                    }
                    k => {
                        if let Some(ch) = k.dial_char()
                            && u.dial_buffer.len() < 24 {
                                u.dial_buffer.push(ch);
                            }
                    }
                });
            }
            let ui = store.get::<st::Ui>();
            p.lock().unwrap().paint(model, &ui.dial_buffer, held, store.get::<st::Audio>().off_hook);
            Ok(Reply::Done)
        }
        Command::InjectHook { off_hook } => {
            sim2.set_hook(*off_hook);
            store.update::<st::Audio, _>(|a| {
                a.off_hook = *off_hook;
                a.path = if *off_hook { st::AudioPath::Handset } else { st::AudioPath::None };
            });
            let ui = store.get::<st::Ui>();
            p.lock().unwrap().paint(model, &ui.dial_buffer, held, *off_hook);
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });

    handle(store, Domain::Calls, |store, c| match c {
        Command::Dial { target, account, .. } => {
            let id = store.next_call_id();
            let party = st::Party { number: target.clone(), ..Default::default() };
            store.update::<st::Calls, _>(|calls| {
                calls.calls.push(st::Call::new(id, account.unwrap_or(st::AccountId(0)), st::Direction::Outgoing, party));
                calls.focused = Some(id);
            });
            let s = store.clone();
            std::thread::spawn(move || {
                let step = |state: st::CallState| {
                    s.update::<st::Calls, _>(|calls| {
                        if let Some(c) = calls.get_mut(id).filter(|c| !c.state.is_ended()) {
                            if state == st::CallState::Active {
                                c.connected_mono_ms = Some(st::clock::mono_ms());
                                c.codec = Some(st::Codec::G722);
                            }
                            c.state = state;
                        }
                    })
                };
                std::thread::sleep(Duration::from_millis(700));
                step(st::CallState::Ringback { early_media: false });
                std::thread::sleep(Duration::from_millis(2500));
                step(st::CallState::Active);
                s.update::<st::Audio, _>(|a| a.path = st::AudioPath::Speaker);
            });
            Ok(Reply::Call(id))
        }
        Command::Hangup { call } => {
            let found = store.update::<st::Calls, _>(|calls| match calls.get_mut(*call) {
                Some(c) => {
                    c.state = st::CallState::Ended(st::EndCause::LocalHangup);
                    true
                }
                None => false,
            });
            if !found {
                return Err(CommandError::Invalid("no such call".into()));
            }
            let (s, id) = (store.clone(), *call);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                s.update::<st::Calls, _>(|calls| {
                    calls.calls.retain(|c| c.id != id);
                    if calls.focused == Some(id) {
                        calls.focused = None;
                    }
                });
                s.update::<st::Audio, _>(|a| a.path = st::AudioPath::None);
            });
            Ok(Reply::Done)
        }
        Command::Hold { call } | Command::Resume { call } => {
            let hold = matches!(c, Command::Hold { .. });
            store.update::<st::Calls, _>(|calls| {
                if let Some(c) = calls.get_mut(*call) {
                    c.state = if hold { st::CallState::Held { local: true, remote: false } } else { st::CallState::Active };
                }
            });
            Ok(Reply::Done)
        }
        Command::Dtmf { .. } | Command::Reregister { .. } => Ok(Reply::Done),
        Command::SetDnd { on } => {
            store.update::<st::Features, _>(|f| f.dnd = *on);
            Ok(Reply::Done)
        }
        Command::SetForward { kind, target, no_answer_secs } => {
            store.update::<st::Features, _>(|f| {
                match kind {
                    st::ForwardKind::Always => f.forward.always = target.clone(),
                    st::ForwardKind::Busy => f.forward.busy = target.clone(),
                    st::ForwardKind::NoAnswer => f.forward.no_answer = target.clone(),
                }
                if let Some(s) = no_answer_secs {
                    f.forward.no_answer_secs = *s;
                }
            });
            Ok(Reply::Done)
        }
        Command::ClearCallLog => {
            store.update::<st::CallLog, _>(|l| *l = Default::default());
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });

    handle(store, Domain::Audio, |store, c| match c {
        Command::SetMute { on } => {
            store.update::<st::Audio, _>(|a| a.muted = *on);
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });

    handle(store, Domain::Config, move |store, c| match c {
        Command::ReloadConfig => {
            let merged = config.lock().unwrap().config();
            let generation = merged.generation();
            // Built-in apps follow `disabled_apps`, as the app runtime does.
            let off = merged.disabled_apps().value;
            store.update::<st::Apps, _>(|a| {
                let mut all: Vec<st::AppInfo> = a.installed.drain(..).chain(a.disabled.drain(..)).collect();
                all.sort_by(|x, y| x.name.cmp(&y.name));
                (a.disabled, a.installed) = all.into_iter().partition(|i| i.builtin && off.contains(&i.name));
            });
            store.update::<st::ConfigInfo, _>(|i| {
                i.generation = generation;
                i.last_merge_unix = st::clock::unix_secs();
            });
            Ok(Reply::Done)
        }
        Command::ReloadContacts => {
            store.update::<st::Contacts, _>(|c| c.generation += 1);
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });

    handle(store, Domain::Provisioning, |store, c| {
        match c {
            Command::ForgetProvisioning | Command::SkipProvisioning => store.update::<st::Provisioning, _>(|p| {
                p.phase = st::ProvisionPhase::Standalone;
                p.server = None;
                p.attached = false;
            }),
            _ => {
                store.update::<st::Provisioning, _>(|p| p.phase = st::ProvisionPhase::FetchingConfig);
                let s = store.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(2));
                    s.update::<st::Provisioning, _>(|p| {
                        p.phase = st::ProvisionPhase::Provisioned;
                        p.attached = true;
                        p.last_success_unix = Some(st::clock::unix_secs());
                    });
                });
            }
        }
        Ok(Reply::Done)
    });

    handle(store, Domain::Network, |_, _| Ok(Reply::Done));

    handle(store, Domain::Paging, |store, c| {
        match c {
            Command::StartPage { id } => store.update::<st::Paging, _>(|p| {
                if let Some(g) = p.broadcasts.iter().find(|g| &g.id == id) {
                    p.activity = st::PageActivity::Sending { group: g.id.clone(), name: g.name.clone() };
                }
            }),
            _ => store.update::<st::Paging, _>(|p| p.activity = st::PageActivity::Idle),
        }
        Ok(Reply::Done)
    });

    // The alarm clock keeps its alarms; nothing rings.
    let mut last_alarm = 4;
    handle(store, Domain::Alarms, move |store, c| {
        let mut reply = Ok(Reply::Done);
        store.update::<st::Alarms, _>(|a| match c {
            Command::SetAlarm { alarm } if alarm.id == 0 => {
                last_alarm += 1;
                a.alarms.push(st::Alarm { id: last_alarm, ..alarm.clone() });
                reply = Ok(Reply::Id(last_alarm.into()));
            }
            Command::SetAlarm { alarm } => {
                if let Some(x) = a.alarms.iter_mut().find(|x| x.id == alarm.id) {
                    *x = alarm.clone();
                }
            }
            Command::RemoveAlarm { id } => a.alarms.retain(|x| x.id != *id),
            Command::SnoozeAlarm => {
                if let Some(r) = a.ringing.take() {
                    a.snoozed = Some((r.id, st::clock::unix_secs() + u64::from(a.snooze_secs)));
                }
            }
            Command::DismissAlarm => {
                a.ringing = None;
                a.snoozed = None;
            }
            _ => {}
        });
        reply
    });

    // Bluetooth: discovery finds two made-up devices; one pairs by passkey
    // comparison and the other asks for a PIN.
    handle(store, Domain::Bluetooth, |store, c| {
        let found = || {
            vec![
                st::BluetoothDevice { addr: "70:BF:92:11:22:33".into(), name: "Evolve2 65".into(), rssi: -48 },
                st::BluetoothDevice { addr: "00:16:94:AA:BB:CC".into(), name: String::new(), rssi: -71 },
            ]
        };
        match c {
            Command::BluetoothDiscovery { on } => {
                store.update::<st::Bluetooth, _>(|b| b.discovering = *on);
                if *on {
                    let s = store.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(1));
                        s.update::<st::Bluetooth, _>(|b| {
                            if b.discovering {
                                b.found = found();
                            }
                        });
                    });
                }
            }
            Command::BluetoothPair { addr } => store.update::<st::Bluetooth, _>(|b| {
                let name = b.found.iter().find(|d| &d.addr == addr).map(|d| d.name.clone()).unwrap_or_default();
                let legacy = name.is_empty();
                b.discovering = false;
                b.pairing = Some(st::BluetoothPairing {
                    addr: addr.clone(),
                    name,
                    passkey: (!legacy).then_some(482_913),
                    pin_needed: legacy,
                });
            }),
            Command::BluetoothAnswerPairing { addr, accept, pin } => {
                let wrong_pin = store.get::<st::Bluetooth>().pairing.as_ref().is_some_and(|p| p.pin_needed)
                    && *accept
                    && pin.as_deref() != Some("0000");
                store.update::<st::Bluetooth, _>(|b| {
                    let Some(p) = b.pairing.take() else { return };
                    b.error.clear();
                    if wrong_pin {
                        b.error = "Pairing failed: wrong PIN".into();
                    } else if *accept && &p.addr == addr {
                        b.found.retain(|d| d.addr != p.addr);
                        b.paired.push(st::BluetoothDevice { addr: p.addr.clone(), name: p.name, rssi: 0 });
                        b.connected = p.addr;
                    }
                });
            }
            Command::BluetoothUnpair { addr } => store.update::<st::Bluetooth, _>(|b| {
                b.paired.retain(|d| &d.addr != addr);
                if &b.connected == addr {
                    b.connected.clear();
                }
            }),
            Command::BluetoothConnect { addr } => store.update::<st::Bluetooth, _>(|b| b.connected = addr.clone()),
            Command::BluetoothDisconnect => store.update::<st::Bluetooth, _>(|b| b.connected.clear()),
            other => return unsupported(other),
        }
        Ok(Reply::Done)
    });

    handle(store, Domain::Apps, |store, c| match c {
        Command::LaunchApp { name } | Command::CloseApp { name } => {
            let launch = matches!(c, Command::LaunchApp { .. });
            store.update::<st::Apps, _>(|a| {
                if let Some(i) = a.installed.iter_mut().find(|i| &i.name == name) {
                    i.running = launch;
                }
                a.foreground = launch.then(|| name.clone());
            });
            Ok(Reply::Done)
        }
        Command::InstallApp { path } => {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            store.update::<st::Apps, _>(|a| {
                a.installed.push(st::AppInfo {
                    name: format!("uploaded{}", a.installed.len()),
                    display_name: format!("Uploaded app ({size} bytes)"),
                    kind: st::AppKind::Foreground,
                    ..Default::default()
                })
            });
            Ok(Reply::Done)
        }
        Command::RemoveApp { name } => {
            let removed = store.update::<st::Apps, _>(|a| {
                let before = a.installed.len();
                a.installed.retain(|i| i.builtin || &i.name != name);
                before != a.installed.len()
            });
            if removed { Ok(Reply::Done) } else { Err(CommandError::Denied("built-in apps cannot be removed".into())) }
        }
        other => unsupported(other),
    });

    handle(store, Domain::Update, |store, c| match c {
        Command::InstallFirmware { path, allow_stock } => {
            let stock = std::fs::read(path).map(|d| d.starts_with(b"STOCK")).unwrap_or(false);
            if stock && !allow_stock {
                return Err(CommandError::Denied("stock image without allow_stock".into()));
            }
            let s = store.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let set = |p: st::UpdatePhase| s.update::<st::Update, _>(|u| u.phase = p);
                s.update::<st::Update, _>(|u| {
                    u.incoming_version = if stock { "2_9_28".into() } else { "3_0_1".into() };
                    u.incoming_is_stock = stock;
                });
                set(st::UpdatePhase::Checking);
                std::thread::sleep(Duration::from_millis(600));
                for pct in (0..=100).step_by(20) {
                    set(st::UpdatePhase::Erasing { percent: pct });
                    std::thread::sleep(Duration::from_millis(120));
                }
                for pct in (0..=100).step_by(25) {
                    set(st::UpdatePhase::WritingKernel { percent: pct });
                    std::thread::sleep(Duration::from_millis(120));
                }
                for pct in (0..=100).step_by(5) {
                    set(st::UpdatePhase::WritingRootfs { percent: pct });
                    std::thread::sleep(Duration::from_millis(120));
                }
                for pct in (0..=100).step_by(10) {
                    set(st::UpdatePhase::Verifying { percent: pct });
                    std::thread::sleep(Duration::from_millis(100));
                }
                set(st::UpdatePhase::Committing);
                std::thread::sleep(Duration::from_millis(400));
                set(st::UpdatePhase::Complete);
                let _ = std::fs::remove_file(path);
            });
            Ok(Reply::Done)
        }
        Command::ConfirmTrial => {
            store.update::<st::Update, _>(|u| u.trial = st::TrialBoot::None);
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });

    handle(store, Domain::System, |store, c| match c {
        Command::Reboot { reason } => {
            log::warn!("simulated reboot ({reason}); nothing happens");
            store.update::<st::System, _>(|s| s.last_reboot_reason = reason.clone());
            Ok(Reply::Done)
        }
        Command::FactoryReset => {
            log::warn!("simulated factory reset; nothing happens");
            Ok(Reply::Done)
        }
        Command::EraseNvdata => {
            if store.get::<st::System>().phase != st::Phase::Recovery {
                return Err(CommandError::Denied("nvdata is mounted; use a factory reset instead".into()));
            }
            log::warn!("simulated nvdata erase; nothing happens");
            Ok(Reply::Done)
        }
        Command::SetTime { unix_secs } => {
            log::info!("simulated clock set to {unix_secs}");
            Ok(Reply::Done)
        }
        other => unsupported(other),
    });
}

/// The web UI's server with a simulated phone behind it
#[derive(clap::Parser)]
struct Args {
    /// The model to simulate
    #[arg(long, default_value = "d60")]
    model: Model,
    /// The HTTP port (on 127.0.0.1)
    #[arg(long = "port", value_name = "PORT", default_value_t = 8480)]
    http: u16,
    /// The HTTPS port (on 127.0.0.1); 0 for no HTTPS
    #[arg(long = "https-port", value_name = "PORT", default_value_t = 8443)]
    https: u16,
    /// The nvdata directory [default: one per model in the temporary directory]
    #[arg(long, value_name = "DIR")]
    nvdata: Option<PathBuf>,
    /// Start with web configuration switched off (`web_ui_enabled` 0)
    #[arg(long)]
    web_disabled: bool,
    /// Start in recovery (nvdata could not be mounted)
    #[arg(long)]
    recovery: bool,
    /// Start with failed and stopped subsystems and a failed provisioning
    #[arg(long)]
    failing: bool,
    /// The web terminal's shell [default: /bin/brush, else /bin/sh]
    #[arg(long, value_name = "PROGRAM", env = "DPHONE_SHELL")]
    shell: Option<PathBuf>,
}

fn main() -> std::io::Result<()> {
    let args = <Args as clap::Parser>::parse();
    let model = args.model;
    let store = Store::new(model);
    let _ = store.install_logger(log::LevelFilter::Debug, false);

    let nv_root = args.nvdata.unwrap_or_else(|| std::env::temp_dir().join(format!("opendeskphone-web-sim-{}", model.name())));
    std::fs::create_dir_all(&nv_root)?;
    let nv = Nvdata::new(&nv_root);
    nv.ensure_layout()?;
    if nv.contact_list_names()?.is_empty() {
        nv.write_contact_list(
            "local.xml",
            br#"<phonebooks><contacts group_name="Office" editable="1" id="office">
<contact id="c1" first_name="Morgan" last_name="Li" organization="Example Co" contact_type="sip" subscribe_to="104">
<numbers><number dial="104" label="Extension" primary="1"/><number dial="5551230104" label="Mobile"/></numbers></contact>
<contact id="c2" first_name="Sam" last_name="Ortiz" organization="Example Co" contact_type="sip">
<numbers><number dial="117" label="Extension" primary="1"/></numbers></contact>
</contacts></phonebooks>"#,
        )?;
    }
    let cfg_model: opendeskphone_config::Model = model.name().parse().expect("same model names");
    let config = ConfigStore::open(nv, cfg_model, std::env::temp_dir()).map_err(|e| std::io::Error::other(e.to_string()))?;
    let config = Arc::new(Mutex::new(config));

    fabricate(&store, model);
    if args.recovery {
        store.update::<st::System, _>(|s| s.phase = st::Phase::Recovery);
    }
    // A static address in the configuration is the address in effect.
    if let Some(addr) = config.lock().unwrap().config().static_ipv4() {
        store.update::<st::Network, _>(|n| {
            n.ipv4.source = st::AddrSource::Static;
            n.ipv4.address = Some(addr.address);
            n.ipv4.prefix_len = addr.prefix_len;
            n.ipv4.gateway = addr.gateway;
            n.ipv4.lease_secs = 0;
            n.ipv4.dhcp_server = None;
        });
    }
    if args.failing {
        store.set_health(st::Subsystem::Sip, st::HealthStatus::Failed("no configuration".into()));
        store.set_health(st::Subsystem::Dpma, st::HealthStatus::Degraded("not responding".into()));
        store.set_health(st::Subsystem::Ssh, st::HealthStatus::Stopped);
        store.update::<st::Health, _>(|h| h.healthy = false);
    }
    let switch = opendeskphone_web::WebSwitch::from_config(&config.lock().unwrap().config());
    if args.web_disabled {
        switch.set(false);
    }
    let hal = Hal::sim(model);
    let sim = hal.sim.clone().expect("sim backend");
    spawn_handlers(&store, hal, config.clone());

    // Something for the log page to tail.
    std::thread::spawn(|| {
        let mut n = 0u32;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            n += 1;
            match n % 5 {
                0 => log::warn!(target: "opendeskphone_sip::register", "account 2: 403 Forbidden; retry in 20 s"),
                1 => log::info!(target: "opendeskphone_net::dhcp", "lease renewed for 10.10.20.57/24"),
                _ => log::debug!(target: "opendeskphone_sip::options", "keepalive {n} answered"),
            }
        }
    });

    let upload_dir = std::env::temp_dir().join(format!("opendeskphone-web-sim-upload-{}", model.name()));
    let options = WebOptions {
        http: Some(SocketAddr::from(([127, 0, 0, 1], args.http))),
        https: (args.https != 0).then(|| SocketAddr::from(([127, 0, 0, 1], args.https))),
        upload_dir: upload_dir.clone(),
        ..Default::default()
    };
    // A stand-in for the updater's EFF label parser: images beginning
    // "STOCK" count as stock, "OPEN" as ours, anything else is refused.
    let sink = FileFirmwareSink::new(upload_dir).with_inspector(move |path| {
        let mut head = [0u8; 5];
        use std::io::Read;
        let n = std::fs::File::open(path)?.read(&mut head)?;
        if head[..n].starts_with(b"STOCK") {
            Ok(FirmwareInfo { version: "2_9_28".into(), model: model.name().into(), is_stock: true })
        } else if head[..n].starts_with(b"OPEN") {
            Ok(FirmwareInfo { version: "3_0_1".into(), model: model.name().into(), is_stock: false })
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no EFF label"))
        }
    });
    let mut server = WebServer::new(store, config, options)
        .with_switch(switch)
        .with_screen(Arc::new(SimScreen(sim)))
        .with_firmware_sink(Box::new(sink));
    if let Some(shell) = &args.shell {
        server = server.with_shell(Arc::new(opendeskphone_web::PtyShellHost::new(shell)));
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let bound = server.bind().await?;
        eprintln!(
            "Open Desk Phone web simulator ({model}): http://{}/{}  (admin / 789), nvdata {}",
            bound.http_addr().map(|a| a.to_string()).unwrap_or_default(),
            bound.https_addr().map(|a| format!("  https://{a}/")).unwrap_or_default(),
            nv_root.display(),
        );
        bound.serve().await
    })
}
