//! Provisioned firmware: deciding whether the server's configuration asks
//! for a different firmware image than the one running, and fetching and
//! installing it.
//!
//! Stock (`bootconfig/firmwareCheck.js`, `curl_update_firmware`) takes the
//! first `<firmware model version url>` of `<firmwares>` for its model,
//! compares the **version string** with its own, and on any difference
//! streams the URL into the flasher and reboots; `<public_firmwares>`
//! gives fallback URLs.
//!
//! This keeps the element, the per-model selection, the URLs, the fallback
//! order, and the reboot, but compares **images, not labels**: the
//! provisioned image is the same as the running one when the kernel CRC
//! and size and the root filesystem CRC and size all match
//! ([`ImageChecksum`]).  The running side comes from the running bank's header
//! in `ctrl`; the provisioned side from the first few hundred bytes of the
//! URL (the OTA header at the start of the decrypted EFF), or from an
//! optional `checksum` attribute on the entry.  A D80 has a boot image and
//! a root file system image in their place: the running side is what its
//! recovery recorded when it installed the image, and the provisioned side
//! is in the metadata at the start of the image's package.  The version
//! string is shown and logged only.
//!
//! When a checksum cannot be obtained there is no update: it is logged and
//! looked at again at the next check.  An image that failed to install, or
//! that was installed and is still not what runs (the trial boot went
//! back), is remembered by its checksum and not tried again until the
//! provisioned image changes.

use crate::xml;
use opendeskphone_config::nvdata::write_atomic;
use std::path::{Path, PathBuf};
use opendeskphone_state::{Calls, Command, NoticeLevel, Store, Update, UpdatePhase};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What identifies a firmware image: the checksums and sizes of its two
/// components.  Two images are the same exactly when all four values are
/// equal.
pub use dpma_proto::firmware::ImageChecksum;

/// The checksum of the image whose OTA header (or `ctrl` record) is `h`.
pub fn checksum_of(h: &eff::AppHeader) -> ImageChecksum {
    ImageChecksum { kernel_crc: h.kernel_crc, kernel_size: h.kernel_size, rootfs_crc: h.rootfs_crc, rootfs_size: h.rootfs_size }
}

/// One `<firmware>` or `<public_firmware>` element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareEntry {
    pub model: String,
    /// A label for people; it takes no part in the decision.
    pub version: String,
    pub url: String,
    /// The optional `checksum` attribute, when present and well formed.
    pub checksum: Option<ImageChecksum>,
}

/// The firmware entries of a fetched configuration.
pub fn entries(config: &[u8]) -> (Vec<FirmwareEntry>, Vec<FirmwareEntry>) {
    let text = String::from_utf8_lossy(config);
    let list = |tag: &str| {
        xml::tags(&text, tag)
            .into_iter()
            .map(|t| FirmwareEntry {
                model: xml::attr(t, "model").unwrap_or_default(),
                version: xml::attr(t, "version").unwrap_or_default(),
                url: xml::attr(t, "url").unwrap_or_default(),
                checksum: xml::attr(t, "checksum").and_then(|c| c.parse().ok()),
            })
            .collect::<Vec<_>>()
    };
    (list("firmware"), list("public_firmware"))
}

/// The provisioned image for a model: stock's selection (the first
/// `<firmware>` for the model that has a version and a URL), and the
/// `<public_firmware>` URLs for the model as fallbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provisioned {
    pub version: String,
    pub checksum: Option<ImageChecksum>,
    /// The server's URL first, then the public fallbacks.
    pub urls: Vec<String>,
}

pub fn select(firmwares: &[FirmwareEntry], public: &[FirmwareEntry], model: &str) -> Option<Provisioned> {
    let f = firmwares.iter().find(|f| f.model == model && !f.version.is_empty() && !f.url.is_empty())?;
    let mut urls = vec![f.url.clone()];
    urls.extend(public.iter().filter(|p| p.model == model && !p.url.is_empty()).map(|p| p.url.clone()));
    Some(Provisioned { version: f.version.clone(), checksum: f.checksum, urls })
}

/// The start of an image: who it is for and what it contains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    pub model: String,
    pub version: String,
    pub sum: ImageChecksum,
}

/// Reads the start of an EFF stream, a few hundred bytes
/// ([`dpma_proto::firmware::Image::read`]); the caller then drops the
/// stream, which ends the transfer.
pub fn peek(stream: &mut dyn Read) -> Result<ImageInfo, String> {
    let image = dpma_proto::firmware::Image::read(stream)?;
    Ok(ImageInfo { model: image.model, version: image.version, sum: image.checksum })
}

/// Why a download could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    /// HTTP status, when the server answered.
    pub status: Option<u16>,
    pub message: String,
}

/// Opens firmware images for streaming.  [`crate::fetch::HttpSource`] is
/// the real one.
pub trait FirmwareSource: Send + Sync + 'static {
    /// Starts the download; the image is read from the returned stream as
    /// it arrives.  Dropping the stream early ends the transfer.
    fn open(&self, url: &str, user_agent: &str) -> Result<Box<dyn Read + Send>, FetchError>;
}

/// Why an installation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The image was refused before anything was written (wrong model,
    /// not an EFF).
    Refused(String),
    /// Reading the image or writing flash failed; nothing was committed.
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Refused(m) | InstallError::Failed(m) => f.write_str(m),
        }
    }
}

/// The flash side.  [`crate::install::MtdInstaller`] is the real one, over
/// `opendeskphone-update`.
pub trait FirmwareInstaller: Send + Sync + 'static {
    /// What the running image is: the running bank's header in `ctrl`, or
    /// on a D80 the recovery's record.  An error means it cannot be known
    /// (RAM boot, no valid record).
    fn running_image(&self) -> Result<ImageChecksum, String>;
    /// Installs the EFF read from `image` for the next boot.  On error the
    /// running firmware stays the one that boots.
    fn install(&self, image: &mut dyn Read) -> Result<(), InstallError>;
}

/// Identity and timing for the runner.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Model name as in the configuration and the EFF (`D60`).
    pub model: String,
    /// Running version, provisioning form (`3_0_0`); for the User-Agent
    /// and for messages.
    pub running: String,
    /// Colon-separated lowercase MAC, for `%%MAC_ADDRESS%%` in URLs.
    pub mac: String,
    /// Append the MAC to the download User-Agent (stock
    /// `send_mac_with_useragent`).
    pub mac_in_user_agent: bool,
    /// Wait before retrying a server that answered 503, and how often.
    pub busy_retry: Duration,
    pub busy_retries: u32,
    /// How often to look whether calls have ended.
    pub call_poll: Duration,
    /// Pause between a finished installation and the reboot.
    pub reboot_delay: Duration,
}

impl Policy {
    pub fn new(model: &str, running: &str, mac: &str) -> Self {
        Policy {
            model: model.to_string(),
            running: running.to_string(),
            mac: mac.to_string(),
            mac_in_user_agent: false,
            busy_retry: Duration::from_secs(30),
            busy_retries: 4,
            call_poll: Duration::from_secs(2),
            reboot_delay: Duration::from_secs(3),
        }
    }

    /// `Digium-<model>/<version>[/<MAC>]`, as `curl_update_firmware` sends.
    pub fn user_agent(&self) -> String {
        let mut ua = format!("Digium-{}/{}", self.model, self.running);
        if self.mac_in_user_agent {
            ua.push('/');
            ua.push_str(&self.mac.replace(':', "").to_ascii_uppercase());
        }
        ua
    }
}

/// The image last tried, kept across boots: the loop guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub sum: ImageChecksum,
    /// It was written and selected, and the phone rebooted into it.
    pub installed: bool,
    /// Why it is not tried again.
    pub note: String,
}

impl Attempt {
    /// The record in `file` ([`crate::ClientFiles::firmware_attempt`]).
    pub fn load(file: &Path) -> Option<Attempt> {
        let text = String::from_utf8_lossy(&std::fs::read(file).ok()?).into_owned();
        let mut lines = text.lines();
        let mut first = lines.next()?.split_whitespace();
        let sum = first.next()?.parse().ok()?;
        let installed = first.next()? == "installed";
        Some(Attempt { sum, installed, note: lines.next().unwrap_or("").to_string() })
    }

    fn save(&self, file: &Path) {
        let text =
            format!("{} {}\n{}\n", self.sum, if self.installed { "installed" } else { "failed" }, self.note);
        if let Err(e) = write_atomic(file, text.as_bytes()) {
            log::warn!("dpma: cannot record the firmware attempt: {e}");
        }
    }
}

/// How a check ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The configuration provisions no image for this model, or the
    /// provisioned image is the one running.
    Keep,
    /// Installed; the reboot has been requested.
    Installed(ImageChecksum),
    /// The provisioned image is one that already failed to install, or
    /// was installed without becoming the running image.
    GaveUp(String),
    /// A checksum could not be obtained (running or provisioned): no
    /// update; the next check looks again.
    Unknown(String),
    /// The download broke off; nothing was committed and the image is not
    /// held against the server.
    Failed(String),
}

/// A stream that remembers whether the transport failed under it, to tell
/// a broken download from a bad image.
struct Tracked<'a> {
    inner: &'a mut dyn Read,
    broke: bool,
}

impl Read for Tracked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf).inspect_err(|_| self.broke = true)
    }
}

/// The pieces a check needs.
pub struct Runner {
    pub store: Store,
    pub policy: Policy,
    /// The attempt record ([`Attempt`]).
    pub attempt_file: PathBuf,
    pub source: Arc<dyn FirmwareSource>,
    pub installer: Arc<dyn FirmwareInstaller>,
    /// Set while a check is in progress, so two configuration fetches do
    /// not start two downloads.
    pub running: Arc<AtomicBool>,
}

impl Runner {
    /// Checks a fetched configuration and, when it provisions an image
    /// other than the running one, downloads and installs it and requests
    /// the reboot.  Blocks: run it on its own thread.
    pub fn run(&self, config: &[u8]) -> Outcome {
        let (firmwares, public) = entries(config);
        let Some(provisioned) = select(&firmwares, &public, &self.policy.model) else { return Outcome::Keep };
        if self.running.swap(true, Ordering::SeqCst) {
            return Outcome::Unknown("a firmware check is already running".into());
        }
        let outcome = self.check(&provisioned);
        self.running.store(false, Ordering::SeqCst);
        match &outcome {
            Outcome::Unknown(why) => log::warn!("dpma: provisioned firmware not checked: {why}"),
            Outcome::GaveUp(why) => log::error!("dpma: {why}"),
            _ => {}
        }
        outcome
    }

    fn open(&self, url: &str) -> Result<Box<dyn Read + Send>, String> {
        let p = &self.policy;
        let ua = p.user_agent();
        let mut tries = 0;
        loop {
            match self.source.open(url, &ua) {
                Ok(s) => return Ok(s),
                // Stock waits and retries a server that says 503.
                Err(e) if e.status == Some(503) && tries < p.busy_retries => {
                    tries += 1;
                    log::info!("dpma: firmware server busy; retry {tries} in {:?}", p.busy_retry);
                    std::thread::sleep(p.busy_retry);
                }
                Err(e) => return Err(e.message),
            }
        }
    }

    fn same(&self) -> Outcome {
        // Running what is provisioned: the guard has nothing to remember.
        if self.attempt_file.exists() {
            let _ = std::fs::remove_file(&self.attempt_file);
        }
        Outcome::Keep
    }

    fn check(&self, provisioned: &Provisioned) -> Outcome {
        let p = &self.policy;
        let running = match self.installer.running_image() {
            Ok(sum) => sum,
            Err(e) => return Outcome::Unknown(format!("the running image's checksum is not known: {e}")),
        };
        // A checksum in the entry settles "same" without touching the URL.
        if provisioned.checksum == Some(running) {
            return self.same();
        }
        let mut last = String::from("no URL");
        for url in &provisioned.urls {
            let url = url.replace("%%MAC_ADDRESS%%", &p.mac);
            let info = match self.open(&url).and_then(|mut s| peek(&mut s)) {
                Ok(info) => info,
                Err(e) => {
                    log::warn!("dpma: cannot read the firmware header at {url}: {e}");
                    last = format!("{url}: {e}");
                    continue;
                }
            };
            if info.sum == running {
                if provisioned.checksum.is_some() {
                    log::warn!("dpma: the entry's checksum does not describe the image at {url}; the image is the running one");
                }
                return self.same();
            }
            if let Some(a) = Attempt::load(&self.attempt_file).filter(|a| a.sum == info.sum) {
                let why = if a.installed {
                    format!(
                        "provisioned firmware {} ({}) was installed but did not become the running image; not installing it again",
                        provisioned.version,
                        info.sum
                    )
                } else {
                    format!(
                        "provisioned firmware {} ({}) failed to install before ({}); not trying it again",
                        provisioned.version,
                        info.sum,
                        a.note
                    )
                };
                return Outcome::GaveUp(why);
            }
            if info.model != p.model {
                let note = format!("the image is for the {}, this phone is a {}", info.model, p.model);
                log::error!("dpma: firmware at {url} refused: {note}");
                Attempt { sum: info.sum, installed: false, note: note.clone() }.save(&self.attempt_file);
                return Outcome::GaveUp(note);
            }
            log::info!(
                "dpma: provisioned firmware {} ({}) differs from the running image ({}); updating",
                provisioned.version,
                info.sum,
                running
            );
            return self.install(&url, &provisioned.version, &info);
        }
        Outcome::Unknown(format!("the provisioned image's checksum is not known: {last}"))
    }

    fn install(&self, url: &str, version: &str, info: &ImageInfo) -> Outcome {
        let p = &self.policy;
        // Never during a call, and never on top of another update.
        loop {
            let busy = !self.store.get::<Calls>().is_idle()
                || !matches!(
                    self.store.get::<Update>().phase,
                    UpdatePhase::Idle | UpdatePhase::Complete | UpdatePhase::Failed(_)
                );
            if !busy {
                break;
            }
            std::thread::sleep(p.call_poll);
        }
        self.notice(NoticeLevel::Info, &format!("Updating firmware to {version}.  Do not unplug the phone."));
        self.store.update::<Update, _>(|u| {
            u.incoming_version = info.version.clone();
            u.phase = UpdatePhase::Receiving { percent: 0 };
        });
        let fail = |message: String| {
            self.store.update::<Update, _>(|u| u.phase = UpdatePhase::Failed(message.clone()));
            self.notice(NoticeLevel::Error, &format!("Firmware update to {version} failed: {message}."));
        };
        let mut stream = match self.open(url) {
            Ok(s) => s,
            Err(e) => {
                fail(e.clone());
                return Outcome::Failed(e);
            }
        };
        let mut tracked = Tracked { inner: &mut stream, broke: false };
        match self.installer.install(&mut tracked) {
            Ok(()) => {
                Attempt { sum: info.sum, installed: true, note: String::new() }.save(&self.attempt_file);
                log::info!("dpma: firmware {version} installed; rebooting");
                std::thread::sleep(p.reboot_delay);
                self.store.post(Command::Reboot { reason: format!("firmware updated to {version} by provisioning") });
                Outcome::Installed(info.sum)
            }
            Err(e) => {
                let message = e.to_string();
                fail(message.clone());
                if tracked.broke {
                    // The transfer died; the image itself is not at fault.
                    log::warn!("dpma: firmware download from {url} broke off: {message}");
                    return Outcome::Failed(message);
                }
                log::error!("dpma: firmware from {url} was not installed: {message}");
                Attempt { sum: info.sum, installed: false, note: message.clone() }.save(&self.attempt_file);
                Outcome::GaveUp(message)
            }
        }
    }

    fn notice(&self, level: NoticeLevel, text: &str) {
        self.store.post(Command::Notify {
            level,
            title: "Firmware".into(),
            text: text.to_string(),
            timeout_ms: if level == NoticeLevel::Info { 0 } else { 30_000 },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fw(model: &str, version: &str, url: &str) -> FirmwareEntry {
        FirmwareEntry { model: model.into(), version: version.into(), url: url.into(), checksum: None }
    }

    #[test]
    fn parses_entries_and_checksum() {
        let cfg = b"<config><firmwares>\n<firmware model=\"D60\" version=\"2_9_28\" url=\"http://pbx/a.eff\" network_id=\"n\" checksum=\"0a1b2c3d-1500000-ffffffff-39321600\" />\n<firmware model=\"D40\" version=\"2_9_26\" url=\"http://pbx/b.eff\" checksum=\"junk\"/></firmwares>\n<public_firmwares><public_firmware model=\"D60\" version=\"2_9_28\" url=\"http://dl/a.eff\"/></public_firmwares></config>";
        let (f, p) = entries(cfg);
        let sum = ImageChecksum { kernel_crc: 0x0a1b2c3d, kernel_size: 1_500_000, rootfs_crc: 0xffff_ffff, rootfs_size: 39_321_600 };
        assert_eq!(f[0].checksum, Some(sum));
        assert_eq!(f[1].checksum, None, "a malformed checksum is as good as none");
        assert_eq!(p, vec![fw("D60", "2_9_28", "http://dl/a.eff")]);
    }

    #[test]
    fn selection_follows_stock() {
        let f = [fw("D40", "9", "http://x/d40"), fw("D60", "", "http://x/noversion"), fw("D60", "2_9_28", "http://x/d60"), fw("D60", "9", "http://x/later")];
        let p = [fw("D60", "2_9_28", "http://pub/1"), fw("D40", "2_9_28", "http://pub/no"), fw("D60", "2_9_28", "")];
        let s = select(&f, &p, "D60").unwrap();
        assert_eq!(s.version, "2_9_28");
        assert_eq!(s.urls, ["http://x/d60", "http://pub/1"]);
        assert_eq!(select(&f, &p, "D65"), None);
        assert_eq!(select(&[fw("D60", "4", "")], &[], "D60"), None);
        // The model comparison is exact, like stock's.
        assert_eq!(select(&[fw("d60", "4", "http://x")], &[], "D60"), None);
    }

    #[test]
    fn user_agent_shape() {
        let mut p = Policy::new("D60", "3_0_0", "00:0f:d3:aa:bb:cc");
        assert_eq!(p.user_agent(), "Digium-D60/3_0_0");
        p.mac_in_user_agent = true;
        assert_eq!(p.user_agent(), "Digium-D60/3_0_0/000FD3AABBCC");
    }
}
