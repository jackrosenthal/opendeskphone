//! Snapshot harness: renders every native screen for every skin through
//! the HAL simulator and writes PNG files under `ui-snapshots/` in cargo's test scratch directory
//! (`CARGO_TARGET_TMPDIR`, i.e., `<target dir>/tmp/ui-snapshots/`) for
//! eyeballing.  The assertions here are structural (the right screen is
//! up, the frame reached the display); see `tests/behavior.rs` for the
//! behavior tests.

use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::*;
use opendeskphone_ui::appview::{self as av, AppView, WidgetKind};
use opendeskphone_ui::{Ui, UiConfig};
use std::net::Ipv4Addr;
use std::path::PathBuf;

const MONO: u64 = 5_000_000;
/// 2026-11-13 21:58:07 UTC.
const UNIX: u64 = 1_794_607_087;

fn out_dir(model: Model) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-snapshots").join(model.name());
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &std::path::Path, frame: &opendeskphone_hal::Frame, scale: usize) {
    let (w, h) = (frame.info.width as usize, frame.info.height as usize);
    let bpp = frame.info.format.bytes_per_pixel();
    let mut rgb = Vec::with_capacity(w * h * 3 * scale * scale);
    for y in 0..h * scale {
        for x in 0..w * scale {
            let i = (y / scale) * frame.info.stride + (x / scale) * bpp;
            if bpp == 4 {
                rgb.extend_from_slice(&[frame.pixels[i + 2], frame.pixels[i + 1], frame.pixels[i]]);
            } else {
                rgb.extend_from_slice(&[frame.pixels[i]; 3]);
            }
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, (w * scale) as u32, (h * scale) as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

struct Rig {
    model: Model,
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
}

impl Rig {
    fn new(model: Model) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let cfg = UiConfig {
            utc_offset_secs: -7 * 3600,
            ringtone: "Flutey Phone".into(),
            ringtones: vec!["Flutey Phone".into(), "Acheron".into(), "Andromeda".into()],
            ..Default::default()
        };
        let mut ui = Ui::from_hal(&mut hal, store.clone(), cfg);
        ui.set_fixed_time(Some((MONO, UNIX)));
        Rig { model, store, ui, sim }
    }

    fn shot(&mut self, name: &str) {
        self.ui.pump();
        let frame = self.sim.frame(Screen::Main).expect("main frame");
        // Something was drawn: the frame is not one flat color.
        assert!(frame.pixels.iter().any(|p| *p != frame.pixels[0]), "{name}: blank frame");
        let scale = if self.model.is_color() { 1 } else { 2 };
        write_png(&out_dir(self.model).join(format!("{name}.png")), &frame, scale);
        if let Some(side) = self.sim.frame(Screen::Side) {
            write_png(&out_dir(self.model).join(format!("{name}-side.png")), &side, 2);
        }
    }

    fn tap(&mut self, key: Key) {
        self.sim.tap(key);
        self.ui.pump();
    }

    fn show(&mut self, screen: &str) {
        let p = self.store.send(Command::ShowScreen { name: screen.into() });
        self.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done), "{screen}");
    }

    fn running(&mut self) {
        let model = self.model;
        self.store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
            s.mac = Some([0x00, 0x0f, 0xd3, 0x12, 0x34, 0x56]);
            s.hostname = "dphone-123456".into();
            s.mem_total_kib = 113 * 1024;
            s.last_reboot_reason = "firmware update".into();
        });
        self.store.update::<Network, _>(|n| {
            n.phase = NetPhase::Up;
            n.lan = PortLink { up: true, speed: 100, full_duplex: true };
            n.ipv4 = Ipv4State {
                source: AddrSource::Dhcp,
                address: Some(Ipv4Addr::new(198, 18, 15, 87)),
                prefix_len: 24,
                gateway: Some(Ipv4Addr::new(198, 18, 15, 1)),
                ..Default::default()
            };
            n.dns = vec![Ipv4Addr::new(198, 18, 15, 1).into()];
        });
        self.store.update::<Accounts, _>(|a| {
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
        self.store.update::<Presence, _>(|p| {
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
        self.store.update::<Keys, _>(|k| {
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
        self.store.update::<Mwi, _>(|m| {
            m.accounts.push(MwiAccount { account: AccountId(0), waiting: true, new: 3, old: 1, urgent_new: 0, urgent_old: 0 })
        });
        self.store.update::<CallLog, _>(|l| {
            for (i, (kind, name, number)) in [
                (CallLogKind::Placed, "Alice Example", "1002"),
                (CallLogKind::Received, "", "15555550123"),
                (CallLogKind::Missed, "Bob Builder", "1003"),
                (CallLogKind::Missed, "Carol", "1004"),
                (CallLogKind::Placed, "Parking", "700"),
            ]
            .into_iter()
            .enumerate()
            {
                l.push(CallLogEntry {
                    id: 0,
                    kind,
                    party: Party { name: name.into(), number: number.into(), uri: String::new() },
                    account: AccountId(0),
                    start_unix: UNIX - 86_400 + i as u64 * 4000,
                    duration_secs: 65 * i as u32,
                });
            }
            l.missed_unseen = 0;
        });
        self.store.update::<Apps, _>(|a| {
            for (name, display) in [
                ("contacts", "Contacts"),
                ("voicemail", "Voicemail"),
                ("status", "Status"),
                ("parking", "Parking"),
                ("queues", "Queues"),
                ("hotdesking", "Hot Desking"),
            ] {
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
        self.store.update::<Update, _>(|u| u.running_bank = Some(Bank::B));
        self.store.update::<Audio, _>(|a| a.dsp = DspStatus::Running);
    }

    fn call(&mut self, direction: Direction, state: CallState, name: &str, number: &str) -> CallId {
        let id = self.store.next_call_id();
        self.store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(0), direction, Party { name: name.into(), number: number.into(), uri: String::new() });
            if state.is_established() {
                call.connected_mono_ms = Some(MONO - 83_000);
                call.codec = Some(Codec::G722);
            }
            call.state = state;
            c.calls.push(call);
            c.focused = Some(id);
        });
        id
    }

    fn end_calls(&mut self) {
        self.store.update::<Calls, _>(|c| {
            c.calls.clear();
            c.focused = None;
        });
        self.ui.pump();
    }
}

fn sample_app(model: Model) -> (AppView, opendeskphone_ui::AppEvents) {
    let (view, events) = AppView::new("voicemail", model);
    let m = view.metrics();
    {
        let mut t = view.batch();
        let title = t.create(WidgetKind::Text, None, av::Rect::new(0, 0, m.width, m.title_height));
        t.set_text(title, "Voicemail \u{2013} INBOX");
        t.set_font(title, av::Font::Title);
        t.set_colors(title, av::Color::Theme(av::ThemeColor::TitleForeground), av::Color::Theme(av::ThemeColor::TitleBackground));
        t.set_align(title, av::Align::Center, av::VAlign::Middle);
        let list = t.create(WidgetKind::List, None, av::Rect::new(0, m.title_height as i16, m.width, m.height - m.title_height));
        t.list_set_columns(list, &[av::Column::px(m.row_height + 4), av::Column::rest(), av::Column::percent(30).align(av::Align::Right)]);
        t.list_set_rows(
            list,
            (0..7)
                .map(|i| {
                    vec![
                        av::Cell::icon(if i < 2 { "voicemail" } else { "ok" }),
                        av::Cell::text(["Alice Example", "Bob Builder", "15555550123", "Carol", "Dave", "Erin", "Frank"][i]),
                        av::Cell::text(format!("0:{:02}", 12 + i * 7)),
                    ]
                })
                .collect(),
        );
        t.list_select(list, 1);
        t.focus(Some(list));
        t.set_softkeys(
            None,
            vec![
                av::Softkey::new(1, "Play"),
                av::Softkey::new(2, "Call"),
                av::Softkey::new(3, "Delete"),
                av::Softkey::new(4, "Forward"),
                av::Softkey::new(5, "Folders"),
            ],
        );
    }
    (view, events)
}

fn form_app(model: Model) -> (AppView, opendeskphone_ui::AppEvents) {
    let (view, events) = AppView::new("hotdesking", model);
    let m = view.metrics();
    let mut t = view.batch();
    let lh = m.line_height;
    let label = t.create(WidgetKind::Text, None, av::Rect::new(4, 2, m.width - 8, lh));
    t.set_text(label, "Extension");
    let ih = if m.color { 36 } else { lh + 4 };
    let ext = t.create(WidgetKind::Input, None, av::Rect::new(4, (lh + 4) as i16, m.width - 8, ih));
    t.input_set_mode(ext, av::InputMode::Numeric);
    t.set_text(ext, "1042");
    let label2 = t.create(WidgetKind::Text, None, av::Rect::new(4, (lh + ih + 8) as i16, m.width - 8, lh));
    t.set_text(label2, "PIN");
    let pin = t.create(WidgetKind::Input, None, av::Rect::new(4, (2 * lh + ih + 10) as i16, m.width - 8, ih));
    t.input_set_mode(pin, av::InputMode::Pin);
    t.input_set_placeholder(pin, "Required");
    t.focus(Some(ext));
    t.set_softkeys(None, vec![av::Softkey::new(1, "Log in"), av::Softkey::new(2, "Cancel")]);
    drop(t);
    (view, events)
}

fn all_screens(model: Model) {
    let mut r = Rig::new(model);
    // Start-up as the main role reports it, stage by stage.
    r.store.set_health(Subsystem::Config, HealthStatus::Ok);
    r.store.set_health(Subsystem::Media, HealthStatus::Starting);
    r.shot("01-boot");
    r.store.set_health(Subsystem::Media, HealthStatus::Ok);
    r.store.set_health(Subsystem::Sip, HealthStatus::Starting);
    r.shot("01-boot-2-telephony");
    r.store.set_health(Subsystem::Sip, HealthStatus::Ok);
    r.store.set_health(Subsystem::Apps, HealthStatus::Starting);
    r.store.set_health(Subsystem::Web, HealthStatus::Starting);
    r.shot("01-boot-3-apps");
    r.store.set_health(Subsystem::Apps, HealthStatus::Ok);
    r.shot("01-boot-4-web");
    r.store.update::<System, _>(|s| s.phase = Phase::NetworkWait);
    r.shot("02-network-wait");
    r.store.update::<System, _>(|s| s.phase = Phase::Recovery);
    r.shot("03-recovery");
    assert_eq!(r.ui.screen_name(), "recovery");

    // Provisioning.
    r.store.update::<System, _>(|s| s.phase = Phase::Provisioning);
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Discovering);
    r.shot("04-prov-discovering");
    r.store.update::<Provisioning, _>(|p| {
        p.phase = ProvisionPhase::AwaitingServer;
        p.discovered = vec![
            DiscoveredServer { source: DiscoverySource::Mdns, name: "Office PBX".into(), address: "198.18.15.2:5060".into() },
            DiscoveredServer { source: DiscoverySource::DhcpOption66, name: String::new(), address: "pbx.example.invalid".into() },
        ];
    });
    r.shot("05-prov-servers");
    r.tap(Key::Down);
    r.tap(Key::Down);
    r.tap(Key::Select);
    assert_eq!(r.ui.screen_name(), "provision-server");
    for k in [2, 2, 3, 3] {
        r.tap(Key::Digit(k));
    }
    r.shot("06-prov-server-entry");
    r.tap(Key::Cancel);
    r.store.update::<Provisioning, _>(|p| {
        p.phase = ProvisionPhase::AwaitingUser;
        p.users = (0..9)
            .map(|i| ProvisionUser { id: format!("10{i}"), name: format!("User Number {i}"), needs_pin: i % 2 == 0 })
            .collect();
    });
    r.shot("07-prov-users");
    r.tap(Key::Select);
    for k in [1, 2, 3] {
        r.tap(Key::Digit(k));
    }
    r.shot("08-prov-pin");
    r.tap(Key::Cancel);
    r.store.update::<Provisioning, _>(|p| {
        p.phase = ProvisionPhase::Failed { reason: "The server did not answer".into(), code: Some(-398) }
    });
    r.shot("09-prov-failed");
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Provisioned);

    // Idle.
    r.running();
    r.shot("10-idle");
    assert_eq!(r.ui.screen_name(), "idle");
    r.store.update::<Features, _>(|f| {
        f.dnd = true;
        f.forward.always = "1099".into();
    });
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 2);
    r.store.update::<Audio, _>(|a| a.headset_mode = true);
    r.tap(Key::Soft(4)); // More
    r.shot("11-idle-dnd-more");
    r.store.update::<Features, _>(|f| *f = Features::default());
    r.store.update::<Audio, _>(|a| a.headset_mode = false);
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(4));

    // Dialer.
    for k in [5, 5, 5, 0, 1, 2, 3] {
        r.tap(Key::Digit(k));
    }
    assert_eq!(r.ui.screen_name(), "dialer");
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().dial_buffer, "5550123");
    r.shot("12-dialer");
    r.tap(Key::Cancel);

    // Calls.
    let c1 = r.call(Direction::Incoming, CallState::Ringing, "Alice Example", "1002");
    r.shot("13-call-incoming");
    assert_eq!(r.ui.screen_name(), "call");
    r.store.update::<Calls, _>(|c| {
        let call = c.get_mut(c1).unwrap();
        call.state = CallState::Active;
        call.connected_mono_ms = Some(MONO - 83_000);
        call.codec = Some(Codec::G722);
        call.srtp = true;
    });
    r.store.update::<Audio, _>(|a| {
        a.path = AudioPath::Handset;
        a.muted = true;
    });
    r.shot("14-call-active");
    r.tap(Key::VolumeUp);
    r.shot("15-volume");
    r.ui.set_fixed_time(Some((MONO + 3000, UNIX + 3)));
    r.ui.pump();
    r.store.update::<Audio, _>(|a| a.muted = false);
    r.store.update::<Calls, _>(|c| c.get_mut(c1).unwrap().state = CallState::Held { local: true, remote: false });
    r.call(Direction::Outgoing, CallState::Active, "", "15555550123");
    r.shot("16-call-two");
    r.store.update::<Calls, _>(|c| {
        for call in &mut c.calls {
            call.state = CallState::Active;
            call.in_conference = true;
        }
    });
    r.shot("17-call-conference");
    r.end_calls();
    assert_eq!(r.ui.screen_name(), "idle");
    r.call(Direction::Outgoing, CallState::Ringback { early_media: false }, "Bob Builder", "1003");
    r.shot("18-call-outgoing");
    r.end_calls();

    // Menus.
    r.show("call_log");
    r.tap(Key::Down);
    r.tap(Key::Down);
    r.shot("20-call-log");
    r.show("menu");
    r.shot("21-menu");
    r.show("settings");
    r.shot("22-settings");
    r.show("network");
    r.tap(Key::Down);
    r.shot("23-network");
    r.tap(Key::Select);
    r.shot("24-network-edit");
    r.show("about");
    r.shot("25-about");
    r.show("apps");
    r.tap(Key::Down);
    r.shot("26-apps");
    r.show("forward");
    r.shot("27-forward");
    r.show("settings");
    r.tap(Key::Digit(3));
    r.tap(Key::Digit(1));
    r.shot("28-brightness");
    r.show("settings");
    r.tap(Key::Digit(5));
    r.tap(Key::Digit(4));
    r.shot("29-confirm");
    r.show("idle");

    // Notices and update.
    let p = r.store.send(Command::Notify {
        level: NoticeLevel::Warning,
        title: "Registration".into(),
        text: "Account 101 could not register: 403 Forbidden".into(),
        timeout_ms: 0,
    });
    r.shot("31-notice");
    assert!(matches!(p.wait(), Ok(Reply::Id(_))));
    r.tap(Key::Select);
    assert!(r.store.get::<opendeskphone_state::Ui>().notices.is_empty());
    r.store.update::<Update, _>(|u| {
        u.phase = UpdatePhase::WritingRootfs { percent: 42 };
        u.incoming_version = "3_0_1".into();
    });
    r.shot("32-update");
    assert_eq!(r.ui.screen_name(), "update");
    // After the restart into the new image: the same screen, until the
    // trial is confirmed.
    r.store.update::<Update, _>(|u| {
        u.phase = UpdatePhase::Idle;
        u.incoming_version.clear();
        u.trial = TrialBoot::Pending { boots_left: 2 };
    });
    r.shot("33-update-trial");
    assert_eq!(r.ui.screen_name(), "update-trial");
    r.store.update::<Update, _>(|u| u.trial = TrialBoot::None);
    r.ui.pump();
    assert_eq!(r.ui.screen_name(), "idle");

    // Apps.
    let (view, _events) = sample_app(model);
    r.ui.handle().open_app(view);
    r.shot("40-app-list");
    assert_eq!(r.ui.screen_name(), "app:voicemail");
    let (form, _events2) = form_app(model);
    r.ui.handle().open_app(form);
    r.ui.pump();
    r.tap(Key::Digit(7));
    r.shot("41-app-form");
    r.ui.handle().close_app("hotdesking");
    r.ui.handle().close_app("voicemail");
    r.ui.pump();
    assert_eq!(r.ui.screen_name(), "idle");
}

#[test]
fn d40() {
    all_screens(Model::D40);
}
#[test]
fn d50() {
    all_screens(Model::D50);
}
#[test]
fn d60() {
    all_screens(Model::D60);
}
#[test]
fn d65() {
    all_screens(Model::D65);
}
#[test]
fn d70() {
    all_screens(Model::D70);
}
/// The D80 has no keys, but its screens answer to them (the web mirror
/// and the diagnostics send them), so the same walk draws them all.
#[test]
fn d80() {
    all_screens(Model::D80);
}

#[test]
fn idle_app_view() {
    for model in [Model::D40, Model::D60] {
        let mut r = Rig::new(model);
        r.running();
        let (view, _events) = AppView::new("status", model);
        let area = opendeskphone_ui::idle_app_area(model);
        {
            let mut t = view.batch();
            let lh = area.height / 2;
            let line = t.create(WidgetKind::Text, None, av::Rect::new(0, 0, area.width, lh));
            t.set_text(line, "Status: Available");
            t.set_align(line, av::Align::Center, av::VAlign::Middle);
            let line2 = t.create(WidgetKind::Text, None, av::Rect::new(0, lh as i16, area.width, lh));
            t.set_text(line2, "In the office until 5");
            t.set_align(line2, av::Align::Center, av::VAlign::Middle);
            t.set_colors(line2, av::Color::Theme(av::ThemeColor::Secondary), av::Color::Default);
        }
        r.ui.handle().add_idle_view(view.clone());
        r.shot("50-idle-app");
        // A change to the idle view repaints the idle screen.
        let g = r.sim.frame_generation();
        view.clear();
        r.ui.pump();
        assert!(r.sim.frame_generation() > g);
        r.ui.handle().remove_idle_view("status");
        r.ui.pump();
    }
}

/// The idle screen's two "cannot take calls" icons: registered (neither),
/// link up with no account registered (the crossed handset), and no link
/// (network down, alone).
#[test]
fn idle_service_icons() {
    for model in [Model::D40, Model::D50, Model::D60, Model::D65, Model::D70] {
        let mut r = Rig::new(model);
        r.running();
        r.store.update::<CallLog, _>(|l| l.missed_unseen = 0);
        r.store.update::<Accounts, _>(|a| {
            a.accounts[0].registration = Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: MONO + 60_000 }
        });
        r.shot("60-idle-registered");
        let registered = r.sim.frame(Screen::Main).unwrap().pixels;

        // The address is being replaced (the link stays up) and the
        // registrations are gone.
        r.store.update::<Network, _>(|n| {
            n.phase = NetPhase::Configuring;
            n.ipv4 = Ipv4State::default();
        });
        r.store.update::<Accounts, _>(|a| {
            for acct in &mut a.accounts {
                acct.registration = Registration::Unregistered;
            }
        });
        r.shot("61-idle-unregistered");
        let unregistered = r.sim.frame(Screen::Main).unwrap().pixels;

        r.store.update::<Network, _>(|n| {
            n.phase = NetPhase::Down;
            n.lan = PortLink::default();
        });
        r.shot("62-idle-link-down");
        let link_down = r.sim.frame(Screen::Main).unwrap().pixels;

        assert!(registered != unregistered, "{model:?}: no icon for no registration");
        assert!(unregistered != link_down, "{model:?}: the two icons are the same");
        assert!(registered != link_down, "{model:?}: no icon for no link");
    }
}

/// The Bluetooth application (D65) in each of its states: searching with
/// nothing found yet, devices found, a pairing under way, a pairing
/// question, a PIN to type, paired headsets while the search goes on and
/// after it, more devices than fit, a search that found nothing, a pairing
/// that failed, and no controller.
#[test]
fn bluetooth() {
    let mut r = Rig::new(Model::D65);
    r.running();
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 0);
    r.ui.pump();
    let dev = |addr: &str, name: &str, rssi: i8| BluetoothDevice { addr: addr.into(), name: name.into(), rssi };
    r.store.update::<Bluetooth, _>(|b| {
        b.present = true;
        b.powered = true;
        b.discovering = true;
    });
    r.show("bluetooth");
    r.shot("80-bt-searching");
    r.store.update::<Bluetooth, _>(|b| {
        b.found = vec![
            dev("00:1B:DC:0F:A1:02", "Jabra Evolve 65", -48),
            dev("5C:F3:70:11:22:33", "Plantronics Voyager", -63),
            dev("F4:4E:FD:44:55:66", "", -80),
        ];
    });
    r.shot("81-bt-found");
    r.tap(Key::Down);
    r.shot("82-bt-found-second");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing_with = "5C:F3:70:11:22:33".into();
        b.discovering = false;
    });
    r.shot("83-bt-pairing");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing = Some(BluetoothPairing { addr: "5C:F3:70:11:22:33".into(), name: "Plantronics Voyager".into(), passkey: Some(42), pin_needed: false });
    });
    r.show("bluetooth");
    r.shot("84-bt-confirm");
    r.store.update::<Bluetooth, _>(|b| b.pairing.as_mut().unwrap().pin_needed = true);
    r.show("bluetooth");
    r.shot("85-bt-pin-question");
    for d in [0, 0, 0] {
        r.tap(Key::Digit(d));
    }
    r.shot("86-bt-pin");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing = None;
        b.pairing_with.clear();
        b.paired = vec![dev("5C:F3:70:11:22:33", "Plantronics Voyager", 0), dev("00:0D:44:AA:BB:CC", "Office Headset", 0)];
        b.connected = "5C:F3:70:11:22:33".into();
        b.found.retain(|d| d.addr != "5C:F3:70:11:22:33");
        b.discovering = true;
    });
    r.show("bluetooth");
    r.shot("87-bt-paired-searching");
    r.store.update::<Bluetooth, _>(|b| b.discovering = false);
    r.tap(Key::Down);
    r.shot("88-bt-paired-done");
    r.store.update::<Bluetooth, _>(|b| {
        for i in 0..6 {
            b.found.push(dev(&format!("00:00:00:00:00:0{i}"), &format!("Headset {}", i + 1), -85));
        }
    });
    for _ in 0..6 {
        r.tap(Key::Down);
    }
    r.shot("89-bt-scrolled");
    r.store.update::<Bluetooth, _>(|b| {
        b.found.clear();
        b.paired.clear();
        b.connected.clear();
        b.discovering = false;
    });
    r.shot("90-bt-nothing-found");
    r.store.update::<Bluetooth, _>(|b| b.error = "pairing with Plantronics Voyager failed".into());
    r.shot("91-bt-error");
    r.store.update::<Bluetooth, _>(|b| *b = Bluetooth::default());
    r.show("bluetooth");
    r.shot("92-bt-no-controller");
}

/// The alarm clock's native screens: the idle screen with the icon of an
/// upcoming alarm, and the alarm that rings.
#[test]
fn alarm_clock() {
    for model in [Model::D40, Model::D50, Model::D60, Model::D65, Model::D70] {
        let mut r = Rig::new(model);
        r.running();
        r.store.update::<CallLog, _>(|l| l.missed_unseen = 0);
        r.store.update::<Accounts, _>(|a| {
            a.accounts[0].registration = Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: MONO + 60_000 }
        });
        r.ui.pump();
        let plain = r.sim.frame(Screen::Main).unwrap().pixels;
        r.store.update::<Alarms, _>(|a| {
            a.alarms = vec![Alarm { id: 1, hour: 8, minute: 0, days: 0b0111110, tone: "Osmium".into(), enabled: true }];
            a.icon = true;
        });
        r.shot("70-idle-alarm-icon");
        assert!(plain != r.sim.frame(Screen::Main).unwrap().pixels, "{model:?}: no icon for an upcoming alarm");
        r.store.update::<Alarms, _>(|a| a.ringing = Some(AlarmRinging { id: 1, hour: 8, minute: 0 }));
        r.ui.pump();
        assert_eq!(r.ui.screen_name(), "alarm");
        r.shot("71-alarm-ringing");
    }
}

/// The D80's Bluetooth application in the states of `bluetooth`: each
/// device's row has its buttons, the pairing question has its own, and a
/// finger scrolls the list.
#[test]
fn bluetooth_d80() {
    let mut r = Rig::new(Model::D80);
    r.running();
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 0);
    r.ui.pump();
    let dev = |addr: &str, name: &str, rssi: i8| BluetoothDevice { addr: addr.into(), name: name.into(), rssi };
    r.store.update::<Bluetooth, _>(|b| {
        b.present = true;
        b.powered = true;
        b.discovering = true;
    });
    r.show("bluetooth");
    r.shot("80-bt-searching");
    r.store.update::<Bluetooth, _>(|b| {
        b.found = vec![
            dev("00:1B:DC:0F:A1:02", "Jabra Evolve 65", -48),
            dev("5C:F3:70:11:22:33", "Plantronics Voyager", -63),
            dev("F4:4E:FD:44:55:66", "", -80),
        ];
    });
    r.shot("81-bt-found");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing_with = "5C:F3:70:11:22:33".into();
        b.discovering = false;
    });
    r.shot("83-bt-pairing");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing = Some(BluetoothPairing { addr: "5C:F3:70:11:22:33".into(), name: "Plantronics Voyager".into(), passkey: Some(42), pin_needed: false });
    });
    r.show("bluetooth");
    r.shot("84-bt-confirm");
    r.store.update::<Bluetooth, _>(|b| b.pairing.as_mut().unwrap().pin_needed = true);
    r.show("bluetooth");
    r.shot("85-bt-pin-question");
    r.store.update::<Bluetooth, _>(|b| {
        b.pairing = None;
        b.pairing_with.clear();
        b.paired = vec![dev("5C:F3:70:11:22:33", "Plantronics Voyager", 0), dev("00:0D:44:AA:BB:CC", "Office Headset", 0)];
        b.connected = "5C:F3:70:11:22:33".into();
        b.found.retain(|d| d.addr != "5C:F3:70:11:22:33");
        b.discovering = true;
    });
    r.show("bluetooth");
    r.shot("87-bt-paired-searching");
    r.store.update::<Bluetooth, _>(|b| b.discovering = false);
    r.shot("88-bt-paired-done");
    r.store.update::<Bluetooth, _>(|b| {
        for i in 0..6 {
            b.found.push(dev(&format!("00:00:00:00:00:0{i}"), &format!("Headset {}", i + 1), -85));
        }
    });
    r.sim.touch_drag((400, 1000), (400, 300), 8);
    r.shot("89-bt-scrolled");
    r.store.update::<Bluetooth, _>(|b| {
        b.found.clear();
        b.paired.clear();
        b.connected.clear();
        b.discovering = false;
    });
    r.shot("90-bt-nothing-found");
    r.store.update::<Bluetooth, _>(|b| b.error = "pairing with Plantronics Voyager failed".into());
    r.shot("91-bt-error");
    r.store.update::<Bluetooth, _>(|b| *b = Bluetooth::default());
    r.show("bluetooth");
    r.shot("92-bt-no-controller");
}

/// The D80's ringing alarm: the time, and two large buttons.
#[test]
fn alarm_clock_d80() {
    let mut r = Rig::new(Model::D80);
    r.running();
    r.store.update::<Alarms, _>(|a| {
        a.alarms = vec![Alarm { id: 1, hour: 8, minute: 0, days: 0b0111110, tone: "Osmium".into(), enabled: true }];
        a.icon = true;
        a.ringing = Some(AlarmRinging { id: 1, hour: 8, minute: 0 });
    });
    r.ui.pump();
    assert_eq!(r.ui.screen_name(), "alarm");
    r.shot("71-alarm-ringing");
}
