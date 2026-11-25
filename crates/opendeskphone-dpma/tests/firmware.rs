//! Provisioned firmware end to end: the decision by image checksum, a
//! fake download source, and the real updater on the HAL's simulated
//! flash.  After every scenario the flash must still boot.
#![cfg(feature = "update")]

mod common;
#[cfg(feature = "https")]
#[path = "common/https.rs"]
mod https;

use common::TempFiles;
use opendeskphone_dpma::firmware::{
    checksum_of, Attempt, FetchError, FirmwareSource, ImageChecksum, Outcome, Policy, Runner,
};
use opendeskphone_dpma::install::MtdInstaller;
use opendeskphone_hal::Hal;
use opendeskphone_state::{Bank, Call, CallId, Calls, Command, Direction, Domain, Model, Party, Store};
use opendeskphone_update::{boot2, ctrl};
use eff::ctrl::{CtrlRecord, Side};
use eff::EffHeader;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BLOCK: usize = 128 * 1024;
const CMDLINE_A: &str = "console=ttyAMA0,115200n8 root=/dev/mtdblock6 rootfstype=jffs2 ro";
const CMDLINE_B: &str = "console=ttyAMA0,115200n8 root=/dev/mtdblock7 rootfstype=jffs2 ro";

fn prng(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

fn kernel(seed: u32) -> Vec<u8> {
    let len = 300_000;
    let mut k = prng(seed, len);
    k[0x24..0x28].copy_from_slice(&boot2::ZIMAGE_MAGIC.to_le_bytes());
    k[0x28..0x2c].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    k[0x2c..0x30].copy_from_slice(&(0x0200_0000u32 + len as u32).to_le_bytes());
    k
}

/// An EFF for `model` labeled `version`, with contents chosen by `seed`.
fn image(model: &str, version: &str, seed: u32) -> (Vec<u8>, ImageChecksum) {
    let (k, r) = (kernel(seed), prng(seed + 100, 2 * BLOCK + 17));
    let ota = eff::ota::pack(&k, &r, "now", &format!("{version}_{model}_firmware.bin")).unwrap();
    let sum = checksum_of(&eff::ota::Ota::parse(&ota).unwrap().header.app);
    (eff::envelope::pack(&EffHeader::new(model, version).unwrap(), &ota, [7; 8]).unwrap(), sum)
}

/// What a URL answers.
#[derive(Clone)]
enum Served {
    Image(Vec<u8>),
    /// The body breaks off after this many bytes with a transport error.
    Broken(Vec<u8>, usize),
    Status(u16),
    Unreachable,
}

#[derive(Default)]
struct FakeSource {
    urls: Mutex<HashMap<String, Served>>,
    /// Every open: URL and User-Agent.
    opens: Mutex<Vec<(String, String)>>,
}

struct Breaking(std::io::Cursor<Vec<u8>>, usize);
impl Read for Breaking {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0.position() as usize >= self.1 {
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset"));
        }
        let left = self.1 - self.0.position() as usize;
        let n = buf.len().min(left);
        self.0.read(&mut buf[..n])
    }
}

impl FirmwareSource for FakeSource {
    fn open(&self, url: &str, user_agent: &str) -> Result<Box<dyn Read + Send>, FetchError> {
        self.opens.lock().unwrap().push((url.to_string(), user_agent.to_string()));
        match self.urls.lock().unwrap().get(url).cloned() {
            Some(Served::Image(b)) => Ok(Box::new(std::io::Cursor::new(b))),
            Some(Served::Broken(b, at)) => Ok(Box::new(Breaking(std::io::Cursor::new(b), at))),
            Some(Served::Status(s)) => Err(FetchError { status: Some(s), message: format!("HTTP {s}") }),
            Some(Served::Unreachable) | None => Err(FetchError { status: None, message: "cannot connect".into() }),
        }
    }
}

struct Rig {
    store: Store,
    sim: opendeskphone_hal::sim::SimHandle,
    mtd: Arc<Mutex<Box<dyn opendeskphone_hal::Mtd>>>,
    source: Arc<FakeSource>,
    files: TempFiles,
    running: ImageChecksum,
    /// The EFF of the image in the running bank.
    running_eff: Vec<u8>,
    dir: std::path::PathBuf,
    reboots: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    /// A D60 running image "3_0_0" (seed 1) from bank A; bank B empty.
    fn new() -> Rig {
        let model = Model::D60;
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().expect("sim handle");
        let (k, r) = (kernel(1), prng(101, 2 * BLOCK + 17));
        let (running_eff, running) = image("D60", "3_0_0", 1);
        let ota = eff::ota::pack(&k, &r, "now", "3_0_0_D60_firmware.bin").unwrap();
        sim.mtd_load("k0", &k).unwrap();
        sim.mtd_load("root0", &r).unwrap();
        let rec = CtrlRecord {
            active: 0,
            sides: [Side::from_header(&eff::ota::Ota::parse(&ota).unwrap().header.app), Side::INVALID],
        };
        let repl = opendeskphone_update::replication_for(model.family());
        ctrl::write(hal.mtd.open("ctrl").unwrap().as_mut(), &rec, repl).unwrap();
        let store = Store::new(model);
        // Catch the reboot request.
        let reboots = Arc::new(Mutex::new(Vec::new()));
        let rx = store.handle(Domain::System);
        let seen = reboots.clone();
        std::thread::spawn(move || {
            while let Some(env) = rx.recv() {
                if let Command::Reboot { reason } = &env.command {
                    seen.lock().unwrap().push(reason.clone());
                }
                env.done();
            }
        });
        let dir = std::env::temp_dir().join(format!("opendeskphone-dpma-fw-{}-{:p}", std::process::id(), &sim));
        std::fs::create_dir_all(&dir).unwrap();
        Rig {
            store,
            sim,
            mtd: Arc::new(Mutex::new(hal.mtd)),
            source: Arc::new(FakeSource::default()),
            files: TempFiles::default(),
            running,
            running_eff,
            dir,
            reboots,
        }
    }

    fn serve(&self, url: &str, what: Served) {
        self.source.urls.lock().unwrap().insert(url.to_string(), what);
    }

    fn runner(&self, cmdline: &str) -> Runner {
        let mut policy = Policy::new("D60", "3_0_0", "00:0f:d3:aa:bb:cc");
        policy.reboot_delay = Duration::ZERO;
        policy.call_poll = Duration::from_millis(10);
        policy.busy_retry = Duration::from_millis(10);
        Runner {
            store: self.store.clone(),
            policy,
            attempt_file: self.files.firmware_attempt.clone(),
            source: self.source.clone(),
            installer: Arc::new(MtdInstaller {
                mtd: self.mtd.clone(),
                model: Model::D60,
                cmdline: cmdline.to_string(),
                own_dir: self.dir.clone(),
                store: self.store.clone(),
            }),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    fn run(&self, config: &str) -> Outcome {
        self.runner(CMDLINE_A).run(config.as_bytes())
    }

    /// A check that downloads from `source`, not from the fake one.
    fn run_from(&self, source: Arc<dyn FirmwareSource>, config: &str) -> Outcome {
        let mut runner = self.runner(CMDLINE_A);
        runner.source = source;
        runner.run(config.as_bytes())
    }

    fn opens(&self) -> usize {
        self.source.opens.lock().unwrap().len()
    }

    /// The side boot2 would boot, and whether its kernel loads.
    fn boots(&self) -> (Bank, bool) {
        let mut mtd = self.mtd.lock().unwrap();
        let b = boot2::simulate(mtd.as_mut()).unwrap();
        (b.side, b.kernel.is_ok())
    }

    fn bank_b_untouched(&self) -> bool {
        self.sim.mtd_contents("k1").unwrap().iter().all(|b| *b == 0xff)
            && self.sim.mtd_contents("root1").unwrap().iter().all(|b| *b == 0xff)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config(version: &str, url: &str, extra: &str) -> String {
    format!(
        "<config>\n<firmwares>\n<firmware model=\"D40\" version=\"1\" url=\"http://pbx/d40.eff\" />\n\
         <firmware model=\"D60\" version=\"{version}\" url=\"{url}\" network_id=\"n\" {extra}/>\n</firmwares>\n</config>"
    )
}

#[test]
fn same_image_under_another_version_label_is_kept() {
    let r = Rig::new();
    // The server calls it 9_9_9, but it is the image that is running.
    let (same, sum) = image("D60", "9_9_9", 1);
    assert_eq!(sum, r.running);
    r.serve("http://pbx/fw.eff", Served::Image(same));
    assert_eq!(r.run(&config("9_9_9", "http://pbx/fw.eff", "")), Outcome::Keep);
    assert_eq!(r.opens(), 1, "only the header fetch");
    assert!(r.bank_b_untouched());
    assert_eq!(r.boots(), (Bank::A, true));
    assert!(r.reboots.lock().unwrap().is_empty());
    // No entry for this model at all: nothing, not even a fetch.
    assert_eq!(r.run("<config><firmwares><firmware model=\"D40\" version=\"1\" url=\"http://pbx/x\"/></firmwares></config>"), Outcome::Keep);
    assert_eq!(r.opens(), 1);
}

#[test]
fn different_image_under_the_same_version_label_is_installed() {
    let r = Rig::new();
    // Labeled 3_0_0 like the running image, but another build.
    let (other, sum) = image("D60", "3_0_0", 2);
    assert_ne!(sum, r.running);
    r.serve("http://pbx/00:0f:d3:aa:bb:cc/fw.eff", Served::Image(other));
    let outcome = r.run(&config("3_0_0", "http://pbx/%%MAC_ADDRESS%%/fw.eff", ""));
    assert_eq!(outcome, Outcome::Installed(sum));
    // Header fetch, then the download, with stock's User-Agent.
    let opens = r.source.opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0], ("http://pbx/00:0f:d3:aa:bb:cc/fw.eff".to_string(), "Digium-D60/3_0_0".to_string()));
    assert_eq!(r.boots(), (Bank::B, true), "the new image is selected and loadable");
    for _ in 0..100 {
        if !r.reboots.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(r.reboots.lock().unwrap().len(), 1, "the reboot was requested");
    assert_eq!(Attempt::load(&r.files.firmware_attempt).map(|a| (a.sum, a.installed)), Some((sum, true)));
    assert!(opendeskphone_update::trial::load(&r.dir).is_some(), "the trial boot is armed");

    // The phone comes up from bank B: provisioned and running agree, and
    // the guard forgets the attempt.
    assert_eq!(r.runner(CMDLINE_B).run(config("3_0_0", "http://pbx/%%MAC_ADDRESS%%/fw.eff", "").as_bytes()), Outcome::Keep);
    assert!(!r.files.firmware_attempt.exists());
}

#[test]
fn stock_image_is_installed_when_provisioned() {
    let r = Rig::new();
    let (stock, sum) = image("D60", "2_9_28", 3);
    r.serve("http://pbx/2_9_28_D60_firmware.eff", Served::Image(stock));
    assert_eq!(r.run(&config("2_9_28", "http://pbx/2_9_28_D60_firmware.eff", "")), Outcome::Installed(sum));
    assert_eq!(r.boots(), (Bank::B, true));
    assert!(r.store.get::<opendeskphone_state::Update>().incoming_is_stock);
}

#[test]
fn checksum_attribute_avoids_the_fetch_but_is_not_trusted_to_flash() {
    let r = Rig::new();
    // Equal to the running image: nothing is fetched.
    let same = format!("checksum=\"{}\" ", r.running);
    assert_eq!(r.run(&config("7", "http://pbx/fw.eff", &same)), Outcome::Keep);
    assert_eq!(r.opens(), 0);
    // A stale attribute says "different", but the image at the URL is the
    // running one: the image decides, and nothing is flashed.
    r.serve("http://pbx/fw.eff", Served::Image(r.running_eff.clone()));
    assert_eq!(r.run(&config("7", "http://pbx/fw.eff", "checksum=\"00000001-2-00000003-4\" ")), Outcome::Keep);
    assert_eq!(r.opens(), 1);
    assert!(r.bank_b_untouched());
}

#[test]
fn unreachable_url_means_no_update_and_fallback_urls_are_tried() {
    let r = Rig::new();
    r.serve("http://pbx/fw.eff", Served::Unreachable);
    let outcome = r.run(&config("4", "http://pbx/fw.eff", ""));
    assert!(matches!(outcome, Outcome::Unknown(_)), "{outcome:?}");
    assert!(r.bank_b_untouched());
    assert!(!r.files.firmware_attempt.exists(), "an unreachable server is not held against the image");
    // Something that is not firmware at the URL: also unknown.
    r.serve("http://pbx/fw.eff", Served::Image(b"<html>404</html>".repeat(100)));
    assert!(matches!(r.run(&config("4", "http://pbx/fw.eff", "")), Outcome::Unknown(_)));
    // A busy server (503) is retried, then given up on for this check.
    r.serve("http://pbx/fw.eff", Served::Status(503));
    let before = r.opens();
    assert!(matches!(r.run(&config("4", "http://pbx/fw.eff", "")), Outcome::Unknown(_)));
    assert_eq!(r.opens() - before, 5, "one try and four retries");
    assert_eq!(r.boots(), (Bank::A, true));

    // With a public fallback that works, the update comes from there.
    let (other, sum) = image("D60", "4", 4);
    r.serve("http://dl/fw.eff", Served::Image(other));
    let cfg = "<config><firmwares><firmware model=\"D60\" version=\"4\" url=\"http://pbx/fw.eff\"/></firmwares>\
               <public_firmwares><public_firmware model=\"D60\" version=\"4\" url=\"http://dl/fw.eff\"/></public_firmwares></config>";
    assert_eq!(r.run(cfg), Outcome::Installed(sum));
    assert_eq!(r.boots(), (Bank::B, true));
}

#[test]
fn wrong_model_is_refused_before_anything_is_written() {
    let r = Rig::new();
    let (d40, sum) = image("D40", "5", 5);
    r.serve("http://pbx/fw.eff", Served::Image(d40));
    let outcome = r.run(&config("5", "http://pbx/fw.eff", ""));
    assert!(matches!(&outcome, Outcome::GaveUp(why) if why.contains("D40")), "{outcome:?}");
    assert!(r.bank_b_untouched());
    assert_eq!(r.boots(), (Bank::A, true));
    assert_eq!(r.opens(), 1, "refused from the header alone");
    assert_eq!(Attempt::load(&r.files.firmware_attempt).map(|a| (a.sum, a.installed)), Some((sum, false)));
    // And it is not looked at twice.
    assert!(matches!(r.run(&config("5", "http://pbx/fw.eff", "")), Outcome::GaveUp(_)));
    assert!(r.reboots.lock().unwrap().is_empty());
}

#[test]
fn corrupt_image_leaves_the_phone_bootable_and_is_not_retried() {
    let r = Rig::new();
    let (mut bad, sum) = image("D60", "6", 6);
    // Damage the root filesystem part; the headers still parse.
    let at = bad.len() - BLOCK;
    bad[at] ^= 0x55;
    r.serve("http://pbx/fw.eff", Served::Image(bad));
    let outcome = r.run(&config("6", "http://pbx/fw.eff", ""));
    assert!(matches!(outcome, Outcome::GaveUp(_)), "{outcome:?}");
    assert_eq!(r.boots(), (Bank::A, true), "the running bank still boots");
    assert!(r.reboots.lock().unwrap().is_empty());
    assert!(matches!(r.store.get::<opendeskphone_state::Update>().phase, opendeskphone_state::UpdatePhase::Failed(_)));
    assert_eq!(Attempt::load(&r.files.firmware_attempt).map(|a| (a.sum, a.installed)), Some((sum, false)));
    // The same image again: only the header is fetched, nothing written.
    let (opens, erases) = (r.opens(), r.sim.mtd_erase_count("root1", 0));
    assert!(matches!(r.run(&config("6", "http://pbx/fw.eff", "")), Outcome::GaveUp(_)));
    assert_eq!(r.opens() - opens, 1);
    assert_eq!(r.sim.mtd_erase_count("root1", 0), erases);
    // A different image under the same label is a new image: installed.
    let (good, good_sum) = image("D60", "6", 17);
    r.serve("http://pbx/fw.eff", Served::Image(good));
    assert_eq!(r.run(&config("6", "http://pbx/fw.eff", "")), Outcome::Installed(good_sum));
    assert_eq!(r.boots(), (Bank::B, true));
}

#[test]
fn broken_download_is_not_held_against_the_image() {
    let r = Rig::new();
    let (good, sum) = image("D60", "8", 8);
    r.serve("http://pbx/fw.eff", Served::Broken(good.clone(), good.len() / 2));
    let outcome = r.run(&config("8", "http://pbx/fw.eff", ""));
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(r.boots(), (Bank::A, true));
    assert!(!r.files.firmware_attempt.exists());
    // The next check gets the whole image and installs it.
    r.serve("http://pbx/fw.eff", Served::Image(good));
    assert_eq!(r.run(&config("8", "http://pbx/fw.eff", "")), Outcome::Installed(sum));
}

#[test]
fn rolled_back_image_is_not_installed_again() {
    let r = Rig::new();
    let (other, sum) = image("D60", "9", 9);
    r.serve("http://pbx/fw.eff", Served::Image(other));
    assert_eq!(r.run(&config("9", "http://pbx/fw.eff", "")), Outcome::Installed(sum));
    // The reboot into the new image was requested.
    for _ in 0..100 {
        if !r.reboots.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // The trial boot fails and the phone is back on bank A.
    {
        let mut mtd = r.mtd.lock().unwrap();
        opendeskphone_update::select_bank(mtd.as_mut(), Model::D60, Bank::A).unwrap();
    }
    assert_eq!(r.boots(), (Bank::A, true));
    let (opens, erases) = (r.opens(), r.sim.mtd_erase_count("k1", 0));
    let outcome = r.run(&config("9", "http://pbx/fw.eff", ""));
    assert!(matches!(&outcome, Outcome::GaveUp(why) if why.contains("did not become the running image")), "{outcome:?}");
    assert_eq!(r.opens() - opens, 1, "only the header fetch");
    assert_eq!(r.sim.mtd_erase_count("k1", 0), erases, "nothing was written again");
    assert_eq!(r.reboots.lock().unwrap().len(), 1, "no second reboot");
    // Once the server provisions another image, updates resume.
    let (next, next_sum) = image("D60", "10", 10);
    r.serve("http://pbx/fw.eff", Served::Image(next));
    assert_eq!(r.run(&config("10", "http://pbx/fw.eff", "")), Outcome::Installed(next_sum));
}

#[test]
fn waits_for_calls_to_end() {
    let r = Rig::new();
    let (other, sum) = image("D60", "11", 11);
    r.serve("http://pbx/fw.eff", Served::Image(other));
    let call = Call::new(CallId(1), opendeskphone_state::AccountId(0), Direction::Outgoing, Party::default());
    r.store.update::<Calls, _>(|c| c.calls.push(call));
    let runner = r.runner(CMDLINE_A);
    let cfg = config("11", "http://pbx/fw.eff", "");
    let worker = std::thread::spawn(move || runner.run(cfg.as_bytes()));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!worker.is_finished(), "no update during a call");
    assert!(r.bank_b_untouched());
    assert_eq!(r.opens(), 1, "the header was looked at, the download not started");
    r.store.update::<Calls, _>(|c| c.calls.clear());
    assert_eq!(worker.join().unwrap(), Outcome::Installed(sum));
    assert_eq!(r.boots(), (Bank::B, true));
}

#[test]
fn unknown_running_image_means_no_update() {
    let r = Rig::new();
    let (other, _) = image("D60", "12", 12);
    r.serve("http://pbx/fw.eff", Served::Image(other));
    // Booted from neither bank (RAM boot): the running image is unknown.
    let runner = r.runner("console=ttyAMA0 root=/dev/ram0");
    assert!(runner.installer.running_image().is_err());
    assert!(matches!(runner.run(config("12", "http://pbx/fw.eff", "").as_bytes()), Outcome::Unknown(_)));
    assert_eq!(r.opens(), 0);
    assert!(r.bank_b_untouched());
}

/// The real download client, with every name resolving to the local server
/// and a trust policy the test changes.
#[cfg(feature = "https")]
fn https_source(trust: &Arc<Mutex<opendeskphone_dpma::fetch::Trust>>) -> Arc<dyn FirmwareSource> {
    let trust = trust.clone();
    Arc::new(
        opendeskphone_dpma::fetch::HttpSource::with_lookup(Box::new(|_, port| Ok(([127, 0, 0, 1], port).into())))
            .with_trust(Box::new(move || trust.lock().unwrap().clone())),
    )
}

#[cfg(feature = "https")]
#[test]
fn https_server_with_an_unknown_issuer_is_refused_and_nothing_is_written() {
    use opendeskphone_dpma::fetch::Trust;
    const CA: &[u8] = include_bytes!("../../opendeskphone-sip/tests/data/test-ca.pem");
    const OTHER_CA: &[u8] = include_bytes!("../../opendeskphone-sip/tests/data/wild-ca.pem");
    let (other, sum) = image("D60", "4", 4);
    let port = https::https_server(other);
    let cfg = config("4", &format!("https://pbx.test:{port}/fw.eff"), "");

    // The phone trusts other authorities than the server's: no download,
    // no write, a reason that names the certificate, and nothing held
    // against the image.
    let r = Rig::new();
    let trust = Arc::new(Mutex::new(Trust::Roots(OTHER_CA.to_vec())));
    let source = https_source(&trust);
    let outcome = r.run_from(source.clone(), &cfg);
    let Outcome::Unknown(why) = &outcome else { panic!("{outcome:?}") };
    assert!(why.contains("TLS handshake with pbx.test failed") && why.contains("UnknownIssuer"), "{why}");
    assert!(r.bank_b_untouched());
    assert!(!r.files.firmware_attempt.exists());
    assert_eq!(r.boots(), (Bank::A, true));
    assert!(r.reboots.lock().unwrap().is_empty());

    // With the server's authority among the roots, the image is installed.
    *trust.lock().unwrap() = Trust::Roots([OTHER_CA, CA].concat());
    assert_eq!(r.run_from(source, &cfg), Outcome::Installed(sum));
    assert_eq!(r.boots(), (Bank::B, true));

    // `allow_insecure_ssl`: the same server without its authority.
    let r = Rig::new();
    let trust = Arc::new(Mutex::new(Trust::Roots(OTHER_CA.to_vec())));
    let source = https_source(&trust);
    assert!(matches!(r.run_from(source.clone(), &cfg), Outcome::Unknown(_)));
    assert!(r.bank_b_untouched());
    *trust.lock().unwrap() = Trust::Any;
    assert_eq!(r.run_from(source, &cfg), Outcome::Installed(sum));
    assert_eq!(r.boots(), (Bank::B, true));
}
