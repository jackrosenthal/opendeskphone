//! opendeskphone-web: the web UI and its Connect RPC services.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use opendeskphone_hal::{PixelFormat, Screen};
use opendeskphone_state::{HealthStatus, Subsystem};
use opendeskphone_web::{
    FileFirmwareSink, FirmwareInfo, FirmwareSink, FirmwareUpload, MirrorFormat, MirrorFrame, MirrorScreen, PtyShellHost, ScreenSource, WebOptions,
    WebServer, WebSwitch,
};
use log::{error, info, warn};

use super::SharedConfig;
use super::system::ConfigHub;
use super::ui::ScreenTap;
use crate::mainrole::{Context, Options};

/// The web terminal's shell: the same program, environment, and working
/// directory an SSH session gets.
fn shell_host(ctx: &Context) -> PtyShellHost {
    let shell = crate::ssh::session::find_shell(ctx.shell.as_deref()).unwrap_or_else(|| "/bin/sh".to_string());
    let mut host = PtyShellHost::new(&shell)
        .env(crate::paths::DIAG_SOCKET_ENV, ctx.paths.diag_socket())
        .current_dir(ctx.paths.home());
    if !ctx.sim {
        host = host.env_clear().envs(crate::init::shell_environment(&ctx.paths, &shell));
    }
    host
}

struct Mirror(Arc<ScreenTap>);

impl ScreenSource for Mirror {
    fn generation(&self) -> u64 {
        self.0.generation()
    }

    fn capture(&self, screen: MirrorScreen) -> Option<MirrorFrame> {
        let frame = self.0.capture(match screen {
            MirrorScreen::Main => Screen::Main,
            MirrorScreen::Side => Screen::Side,
        })?;
        Some(MirrorFrame {
            width: frame.info.width,
            height: frame.info.height,
            stride: frame.info.stride,
            format: match frame.info.format {
                PixelFormat::Xrgb8888 => MirrorFormat::Xrgb8888,
                PixelFormat::R8 => MirrorFormat::Gray8,
            },
            pixels: frame.pixels,
        })
    }
}

/// Read an uploaded image's label: version, model, and whether it is stock.
fn inspect(path: &std::path::Path) -> std::io::Result<FirmwareInfo> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let reader = eff::EffReader::new(file).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let header = reader.header();
    Ok(FirmwareInfo {
        version: header.version.trim().to_string(),
        model: header.model.trim().to_string(),
        is_stock: opendeskphone_update::is_stock_version(&header.version),
    })
}

/// The firmware upload of the phone: the image goes into the spare bank as
/// it arrives (one erase block in memory at a time; nothing in `/tmp` or
/// `/run`, which are RAM), or on a D80 into nvdata, is verified there, and
/// is not installed until the user confirms the installation.  A bad
/// upload costs the spare bank's old contents and nothing else.
struct FlashFirmwareSink(super::system::Flash);

/// The upload's bytes on their way to the updater's thread.
struct ChunkReader {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl std::io::Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => (self.chunk, self.pos) = (chunk, 0),
                // The upload ended (or was abandoned): end of the stream.
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct FlashUpload {
    tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    worker: Option<std::thread::JoinHandle<Result<opendeskphone_update::Staged, String>>>,
    model: opendeskphone_state::Model,
}

impl FlashUpload {
    /// Ends the stream and takes the updater's verdict.
    fn join(&mut self) -> std::io::Result<opendeskphone_update::Staged> {
        self.tx = None;
        let worker = self.worker.take().ok_or_else(|| std::io::Error::other("the upload has ended"))?;
        match worker.join() {
            Ok(Ok(staged)) => Ok(staged),
            Ok(Err(e)) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(_) => Err(std::io::Error::other("the updater failed")),
        }
    }
}

impl FirmwareSink for FlashFirmwareSink {
    fn begin(&self, _size: Option<u64>) -> std::io::Result<Box<dyn FirmwareUpload>> {
        // The bank an earlier upload was staged in is about to be rewritten.
        *super::system::STAGED.lock().map_err(|_| std::io::Error::other("staged image lock poisoned"))? = None;
        // A few chunks of slack; the sender waits when flash is the slower.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let flash = self.0.clone();
        let worker = std::thread::Builder::new().name("update".into()).spawn(move || {
            let mut mtd = flash.mtd.lock().map_err(|_| "flash lock poisoned".to_string())?;
            let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
            opendeskphone_update::Updater::new(mtd.as_mut(), flash.model, &cmdline, &flash.own_dir)
                .store(flash.store.clone())
                .stage_eff(ChunkReader { rx, chunk: Vec::new(), pos: 0 })
                .map_err(|e| e.to_string())
        })?;
        Ok(Box::new(FlashUpload { tx: Some(tx), worker: Some(worker), model: self.0.model }))
    }

    fn discard(&self, _path: &std::path::Path) {
        // Without the staged image the upload cannot be installed.
        if let Ok(mut staged) = super::system::STAGED.lock() {
            *staged = None;
        }
        if let Ok(mut mtd) = self.0.mtd.lock() {
            opendeskphone_update::Updater::new(mtd.as_mut(), self.0.model, "", &self.0.own_dir).discard_staged();
        }
    }
}

impl FirmwareUpload for FlashUpload {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(chunk.to_vec()).is_ok());
        if sent {
            return Ok(());
        }
        // The updater stopped reading: it refused or failed.  Say why.
        Err(match self.join() {
            Err(e) => e,
            Ok(_) => std::io::Error::new(std::io::ErrorKind::InvalidData, "data after the end of the image"),
        })
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<(std::path::PathBuf, FirmwareInfo)> {
        let staged = self.join()?;
        let info = FirmwareInfo {
            version: staged.version.trim().to_string(),
            model: self.model.to_string(),
            is_stock: staged.is_stock,
        };
        *super::system::STAGED.lock().map_err(|_| std::io::Error::other("staged image lock poisoned"))? = Some(staged);
        Ok((super::system::STAGED_PATH.into(), info))
    }
}

/// Where firmware uploads go: into flash on a phone; into a file (which
/// the installation then reads twice) where there is no flash.
fn firmware_sink(upload_dir: &std::path::Path) -> Box<dyn FirmwareSink> {
    match super::system::flash() {
        Some(flash) => Box::new(FlashFirmwareSink(flash)),
        None => Box::new(FileFirmwareSink::new(upload_dir).with_inspector(inspect)),
    }
}

pub fn start(
    ctx: &Context,
    opts: &Options,
    config: Option<SharedConfig>,
    hub: &ConfigHub,
    tap: Option<Arc<ScreenTap>>,
) {
    let Some(config) = config else {
        error!("web: no configuration; the web UI is not started");
        ctx.store.set_health(Subsystem::Web, HealthStatus::Failed("no configuration".into()));
        return;
    };
    // The server always runs.  `web_ui_enabled` off leaves a page saying
    // that web configuration is disabled and refuses everything else;
    // `metrics_enabled` (Open Desk Phone settings) decides whether `/metrics`
    // is served, independently.  Both follow the configuration as it
    // changes.
    let switch = WebSwitch::from_config(&config.lock().unwrap().config());
    let describe = |s: &WebSwitch| {
        let on = |b| if b { "enabled" } else { "disabled" };
        format!("web configuration {} (web_ui_enabled), metrics {} (metrics_enabled)", on(s.enabled()), on(s.metrics_enabled()))
    };
    info!("web: {}", describe(&switch));
    {
        let switch = switch.clone();
        hub.on_change(move |cfg| {
            let before = (switch.enabled(), switch.metrics_enabled());
            switch.follow(cfg);
            if before != (switch.enabled(), switch.metrics_enabled()) {
                info!("web: {}", describe(&switch));
            }
        });
    }
    let mut options = WebOptions::default();
    if ctx.sim {
        // An unprivileged port on this machine only; no HTTPS listener.
        let port = opts.web_port.unwrap_or(8080);
        options.http = Some(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
        options.https = None;
        options.upload_dir = ctx.paths.run.join("upload");
    } else {
        if let Some(port) = opts.web_port {
            options.http = Some(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)));
        }
    }
    ctx.store.set_health(Subsystem::Web, HealthStatus::Starting);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            let mut server = WebServer::new(ctx.store.clone(), config.clone(), options.clone())
                .with_firmware_sink(firmware_sink(&options.upload_dir))
                .with_switch(switch.clone())
                .with_shell(Arc::new(shell_host(&ctx)));
            if let Some(tap) = &tap {
                server = server.with_screen(Arc::new(Mirror(tap.clone())));
            }
            match server.bind().await {
                Ok(bound) => {
                    info!("web: listening (http {:?}, https {:?})", options.http, options.https);
                    ctx.store.set_health(Subsystem::Web, HealthStatus::Ok);
                    if let Err(e) = bound.serve().await {
                        error!("web: server stopped: {e}");
                    } else {
                        warn!("web: server ended");
                    }
                }
                Err(e) => error!("web: cannot bind: {e}"),
            }
            ctx.store.set_health(Subsystem::Web, HealthStatus::Failed("not listening".into()));
            // An IPv6 wildcard that cannot be bound (no IPv6 in the
            // kernel): fall back to IPv4 for the next attempt.
            for addr in [&mut options.http, &mut options.https].into_iter().flatten() {
                if addr.is_ipv6() && addr.ip().is_unspecified() {
                    *addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, addr.port()));
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use opendeskphone_state::{Bank, Model, Reply, Store};
    use opendeskphone_update::{boot2, ctrl, trial};
    use eff::ctrl::{CtrlRecord, Side};

    use super::*;
    use crate::mainrole::subsystems::system::{self, Flash, STAGED_PATH};

    const BLOCK: usize = 128 * 1024;

    fn bytes(seed: u32, len: usize) -> Vec<u8> {
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

    fn kernel(seed: u32, len: usize) -> Vec<u8> {
        let mut k = bytes(seed, len);
        k[0x24..0x28].copy_from_slice(&boot2::ZIMAGE_MAGIC.to_le_bytes());
        k[0x28..0x2c].copy_from_slice(&0x0200_0000u32.to_le_bytes());
        k[0x2c..0x30].copy_from_slice(&(0x0200_0000u32 + len as u32).to_le_bytes());
        k
    }

    /// A web upload on a phone: the image goes into the spare bank chunk by
    /// chunk and is verified there, nothing is selected until the
    /// installation is confirmed, and a broken upload selects nothing.
    #[test]
    fn an_upload_is_staged_in_flash_and_selected_by_the_install_command() {
        let model = Model::D60;
        let mut hal = opendeskphone_hal::Hal::sim(model);
        // Bank A holds an image and is the one boot2 boots; this test
        // system runs from neither bank, so the upload goes to bank B.
        let (ka, ra) = (kernel(1, 300_000), bytes(2, 3 * BLOCK + 17));
        let old = eff::ota::pack(&ka, &ra, "old", "2_9_28_D60_firmware.bin").unwrap();
        let sim = hal.sim.clone().expect("sim handle");
        sim.mtd_load("k0", &ka).unwrap();
        sim.mtd_load("root0", &ra).unwrap();
        let rec = CtrlRecord { active: 0, sides: [Side::from_header(&eff::ota::Ota::parse(&old).unwrap().header.app), Side::INVALID] };
        ctrl::write(hal.mtd.open("ctrl").unwrap().as_mut(), &rec, opendeskphone_update::replication_for(model.family())).unwrap();
        let (kn, rn) = (kernel(3, 400_000), bytes(4, 5 * BLOCK + 99));
        let ota = eff::ota::pack(&kn, &rn, "new", "3_0_0_D60_firmware.bin").unwrap();
        let image = eff::envelope::pack(&eff::EffHeader::new("D60", "3_0_0").unwrap(), &ota, [7; 8]).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(model);
        let mtd: system::SharedMtd = Arc::new(Mutex::new(std::mem::replace(&mut hal.mtd, Box::new(opendeskphone_hal::Null("mtd")))));
        let sink = FlashFirmwareSink(Flash { mtd: mtd.clone(), model, own_dir: dir.path().to_path_buf(), store: store.clone() });
        let next_boot = || boot2::simulate(mtd.lock().unwrap().as_mut()).unwrap().side;

        // An upload that breaks off: an error, and nothing to install.
        let mut upload = sink.begin(None).unwrap();
        for chunk in image[..image.len() / 2].chunks(16 * 1024) {
            upload.write(chunk).unwrap();
        }
        assert!(upload.finish().is_err());
        assert_eq!(next_boot(), Bank::A);
        let e = system::install(&store, &mtd, model, STAGED_PATH, false, dir.path()).unwrap_err();
        assert!(format!("{e:?}").contains("upload it again"), "{e:?}");

        // The whole image: staged, described, and still not selected.
        let mut upload = sink.begin(Some(image.len() as u64)).unwrap();
        for chunk in image.chunks(16 * 1024) {
            upload.write(chunk).unwrap();
        }
        let (path, info) = upload.finish().unwrap();
        assert_eq!(path.to_str(), Some(STAGED_PATH));
        assert_eq!((info.version.as_str(), info.model.as_str(), info.is_stock), ("3_0_0", "D60", false));
        assert_eq!(next_boot(), Bank::A);
        assert!(trial::load(dir.path()).is_none());
        assert_eq!(boot2::load_kernel(mtd.lock().unwrap().as_mut(), Bank::B).unwrap().unwrap(), kn);

        // Confirmed: selected, with the trial recorded.
        let reply = system::install(&store, &mtd, model, STAGED_PATH, false, dir.path()).unwrap();
        assert!(matches!(reply, Reply::Text(v) if v == "3_0_0"));
        assert_eq!(next_boot(), Bank::B);
        assert_eq!(trial::load(dir.path()).unwrap().previous_bank, Bank::A);
        // And no second upload while that image is on trial.
        let mut upload = sink.begin(None).unwrap();
        let refused = image.chunks(16 * 1024).try_for_each(|c| upload.write(c)).and_then(|()| upload.finish().map(|_| ()));
        assert!(refused.unwrap_err().to_string().contains("dphone update abandon"));
        assert_eq!(next_boot(), Bank::B);
    }
}
