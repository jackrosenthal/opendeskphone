//! Migrates a D80 from the stock firmware to Open Desk Phone.
//!
//! Prior to installing Open Desk Phone on stock firmware, the phone needs re-keyed to accept Open
//! Desk Phone's signing keys instead of Digium's.
//!
//! During provisioning, Open DPMA detects the stock firmware (version <3) when it's installing Open
//! Desk Phone (version >=3).  When this is detected:
//! 1. Open DPMA uploads several files to the phone using upload_file.cgi:
//!    - odp-recovery.img: The new recovery image, extracted from the firmware.
//!    - odp-otacerts.zip: Open Desk Phone's OTA certificates.
//!    - odp-update-firmware.sh: A modified version of the phone's update-firmware.sh which does not
//!      check the package's certificates.
//!    - odp-migrate.sh: A shell script to the phone that will flash the recovery image,
//!      replace /system/bin/update-firmware.sh with odp-update-firmware.sh, and replace
//!      /system/security/otacerts.zip with odp-otacerts.zip.
//! 2. A configuration payload with `lan_port_mode` set to
//!    `auto;sh /data/digium/tmp/odp-migrate.sh` is sent to the phone.  The stock firmware
//!    conveniently executes this script for us.
//! 3. The server posts the image to the stock updater (`update_firmware.cgi`).  The modified updater
//!    then accepts the image, and hands the package to the recovery, and the recovery installs it.
//!
//! If `d80_stock_firmware_migration` is set to `no`, Open DPMA will never attempt this migration.

use dpma_ast::bformat;
use dpma_ast::dpma_bytes::ByteSlice;
use dpma_ast::dpma_log;
use eff::EffReader;
use opendeskphone_recovery::{sig, zip};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The certificate installed into the phone's `otacerts.zip`.
const OTA_CERTIFICATE: &str =
    include_str!("../../../../crates/opendeskphone-image/keys/d80-ota.x509.pem");

/// The only model this migration handles: its stock firmware has the
/// command injection, and it has the recovery partition that the re-key
/// overwrites.
const MODEL: &[u8] = b"D80";

/// The size of the recovery partition.  The staged recovery image is
/// padded to this size so that the payload script can read the whole
/// partition back and compare it against what it wrote.
const RECOVERY_PARTITION_SIZE: usize = 8 << 20;

/// The phone's web server upload endpoint.  `upload_file.cgi` saves each
/// upload under its client-supplied file name in `/data/digium/tmp/` (the
/// stock `TMPDIR`) with mode 0600; `dphonecored` can read the files there
/// because it runs as root.
const UPLOAD_PATH: &str = "/cgi-bin/upload_file.cgi";
const PAYLOAD_NAME: &str = "odp-migrate.sh";
const RECOVERY_NAME: &str = "odp-recovery.img";
const UPDATE_SCRIPT_NAME: &str = "odp-update-firmware.sh";
const OTACERTS_NAME: &str = "odp-otacerts.zip";

/// The `lan_port_mode` value used in a migration document: `auto` for the
/// switch configuration, followed by the staged script, which the stock
/// config daemon executes as root.
pub fn payload_setting() -> &'static [u8] {
    b"auto;sh /data/digium/tmp/odp-migrate.sh"
}

/// The phone's unauthenticated firmware upload endpoint; the stock updater
/// runs on whatever is posted there.
const FIRMWARE_PATH: &str = "/cgi-bin/update_firmware.cgi";
const HTTP_PORT: u16 = 80;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Timeouts for uploading a recovery image and for the updater's
/// response; decrypting and verifying a package takes the phone a while.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const FIRMWARE_TIMEOUT: Duration = Duration::from_secs(120);
/// The config daemon runs the payload within seconds of applying a
/// configuration, so the first hand-off waits that long; the remaining
/// attempts retry in case the phone is slower.
const FIRST_HAND_OFF_WAIT: Duration = Duration::from_secs(12);
const HAND_OFF_ATTEMPTS: u32 = 12;
const HAND_OFF_RETRY: Duration = Duration::from_secs(20);
/// If a phone's configuration is reapplied within this window, its files
/// do not need to be staged again.
const STAGE_TTL: Duration = Duration::from_secs(120);

/// Everything a migration needs, built from the Open Desk Phone image in
/// the firmware package.
#[derive(Debug)]
pub struct Kit {
    /// The recovery image from the package, padded to the size of the
    /// recovery partition.
    pub recovery: Vec<u8>,
    /// The replacement for `/system/bin/update-firmware.sh`.
    pub update_script: Vec<u8>,
    /// An `otacerts.zip` holding the Open Desk Phone certificate.
    pub otacerts: Vec<u8>,
    /// The script the payload setting runs.
    pub payload_script: Vec<u8>,
    /// The image from the firmware package, ready to hand to the stock
    /// updater once the phone's key is ours.
    pub eff: Vec<u8>,
    /// The version the image installs (from its encrypted header).
    pub version: String,
}

/// A built kit, or the reason the kit could not be built.
type Built = Result<Arc<Kit>, String>;

/// A cache entry for the package file: its path, plus the modification
/// time and size it was read at.
type CachedKit = Option<(Vec<u8>, (Option<SystemTime>, u64), Built)>;

/// Cache of the last built kit.
static KIT: Mutex<CachedKit> = Mutex::new(None);

/// Returns the kit built from the firmware package's Open Desk Phone
/// image, or None if `d80_stock_firmware_migration` is disabled, there is no
/// D80 package, or its image cannot be used for migration.  Failures are
/// logged once per change of the file.
pub fn kit() -> Option<Arc<Kit>> {
    let general = crate::config::get();
    let package = crate::firmware::find(MODEL)?;
    if package.file.is_empty() {
        return None;
    }
    // Like `checksum_attr`, require a plain file name in the package
    // directory, not a URL or an absolute path.
    if package.file.starts_with(b"/") || package.file.contains_str("//") {
        return None;
    }
    let path = bformat!(
        "{}/{}",
        general.firmware_package_directory,
        crate::util::truncate(&package.file, 0x1000)
    );
    let path = crate::util::truncate(&path, 0x1000);
    let meta = match std::fs::metadata(crate::util::path(&path)) {
        Ok(m) => m,
        Err(e) => {
            dpma_log!(
                "Migration of a stock {}: {} cannot be read: {}",
                String::from_utf8_lossy(MODEL),
                String::from_utf8_lossy(&path),
                e
            );
            return None;
        }
    };
    let stamp = (meta.modified().ok(), meta.len());
    let mut g = KIT.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some((p, s, built)) if *p == path && *s == stamp => built.clone().ok(),
        _ => {
            let bytes = std::fs::read(crate::util::path(&path));
            let built = match bytes {
                Ok(bytes) => build(&bytes).map(Arc::new),
                Err(e) => Err(e.to_string()),
            };
            if let Err(why) = &built {
                dpma_log!(
                    "Migration of a stock {}: no usable image in {}: {}",
                    String::from_utf8_lossy(MODEL),
                    String::from_utf8_lossy(&path),
                    why
                );
            }
            *g = Some((path, stamp, built.clone()));
            built.ok()
        }
    }
}

/// Builds a kit from the bytes of the firmware package's Open Desk Phone
/// image: extracts the recovery image the package carries, and prepares
/// the scripts and certificate that the re-keying needs.
fn build(image: &[u8]) -> Result<Kit, String> {
    let mut eff = EffReader::new(image).map_err(|e| e.to_string())?;
    let header = eff.header().clone();
    if !header.model.trim().eq_ignore_ascii_case("D80") {
        return Err(format!(
            "the image is for a {}, not for a D80",
            header.model.trim()
        ));
    }
    // Re-keying a phone for a stock image would hand it Digium firmware;
    // only an Open Desk Phone image (version 3 and up) may be migrated.
    if !is_odp_version(&header.version) {
        return Err(format!(
            "{} is not an Open Desk Phone version",
            header.version
        ));
    }
    let mut payload = Vec::new();
    eff.read_to_end(&mut payload).map_err(|e| e.to_string())?;
    // The image consists of the package followed by its signature: the
    // recovery gets the package, and the stock updater gets the whole
    // image.
    let (package, _) = sig::split_image(&payload).map_err(|e| format!("not a D80 image: {e}"))?;

    let len = package.len() as u64;
    let mut archive = zip::Archive::open(std::io::Cursor::new(package), len)?;
    if archive
        .entry(opendeskphone_recovery::install::UPDATE_BINARY)
        .is_none()
    {
        return Err(format!(
            "the package has no {}",
            opendeskphone_recovery::install::UPDATE_BINARY
        ));
    }
    let recovery = archive.read(
        opendeskphone_recovery::RECOVERY_ENTRY,
        RECOVERY_PARTITION_SIZE,
    )?;
    if recovery.get(..8) != Some(b"ANDROID!") {
        return Err("the package's recovery image is not a boot image".into());
    }
    // Padded to the partition size, so the payload script writes the
    // whole image and reads the whole partition back for a byte-for-byte
    // comparison.
    let mut recovery = recovery;
    recovery.resize(RECOVERY_PARTITION_SIZE, 0);

    let update_script = update_firmware_script();
    let payload_script = payload_script();
    let otacerts = zip::write(&[zip::FileEntry {
        name: "d80-ota.x509.pem",
        data: OTA_CERTIFICATE.as_bytes(),
        executable: false,
    }])
    .map_err(|e| format!("otacerts.zip: {e}"))?;

    Ok(Kit {
        recovery,
        update_script,
        otacerts,
        payload_script,
        eff: image.to_vec(),
        version: header.version,
    })
}

/// Whether a version string (such as `3_1_0`) is an Open Desk Phone
/// version.
fn is_odp_version(version: &str) -> bool {
    let mut it = version.trim().split(['_', '.']);
    match it.next().and_then(|m| m.parse::<u32>().ok()) {
        Some(major) => major >= 3,
        None => false,
    }
}

/// Returns a kit if the phone behind this transaction should be migrated:
/// it runs stock firmware for [`MODEL`] (its user agent names a version
/// below 3), and the firmware package contains an Open Desk Phone image
/// for it.
pub fn plan(tx: &crate::session::Transaction) -> Option<Arc<Kit>> {
    if !crate::config::get().d80_stock_firmware_migration {
        return None;
    }
    // The user agent looks like "Digium D80 1_12_13": vendor, model, then
    // version.  (`phone_model` also accepts "sangoma p" as a vendor, but
    // no Sangoma phone runs this stock firmware.)
    let (vendor, rest) = tx.user_agent.split_once_str(" ")?;
    if !vendor.eq_ignore_ascii_case(b"Digium") {
        return None;
    }
    let (model, rest) = rest.split_once_str(" ")?;
    if !model.eq_ignore_ascii_case(MODEL) {
        return None;
    }
    let version = rest.split(|&c| c == b' ').next().unwrap_or(b"");
    if !is_stock_version(version) {
        return None;
    }
    kit()
}

/// Whether a version string names a stock release: stock releases are 1.x
/// and 2.x, and Open Desk Phone starts at 3.  Anything that does not
/// parse as a version is not treated as stock, so a phone is never
/// migrated on a guess.
fn is_stock_version(version: &[u8]) -> bool {
    let text = match std::str::from_utf8(version.trim_ascii()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let mut it = text.split(['_', '.']);
    match it.next().and_then(|m| m.parse::<u32>().ok()) {
        Some(major) => major < 3,
        None => false,
    }
}

/// The IP address of the phone's web server: taken from the host of the
/// response URI, or from the session's contact.  A host name is not
/// usable, since this module has no DNS resolver; a phone that is
/// reachable only by name is not migrated.
pub fn phone_ip(tx: &crate::session::Transaction) -> Option<IpAddr> {
    let contact = tx
        .session
        .as_ref()
        .map(|s| s.lock().contact.clone())
        .unwrap_or_default();
    for uri in [&tx.response_uri, &contact] {
        // The address is what follows the scheme and any user part, up to
        // the first parameter.  A response URI has no user part
        // (`pjsip:10.0.0.5:5060;transport=udp`), which `to_host_port`
        // does not split.
        let uri = dpma_proto::uri::get_in_brackets(uri);
        let (addr, _) = dpma_proto::uri::phone_utils_parse_uri(&uri);
        let (host, _) = dpma_proto::uri::split_hostport(&addr);
        let Ok(text) = std::str::from_utf8(&host) else {
            continue;
        };
        match text.trim().parse::<IpAddr>() {
            Ok(ip) => return Some(ip),
            Err(_) => continue,
        }
    }
    None
}

/// For each phone, when its files were last staged and with which kit.
type StagedKits = Option<HashMap<Vec<u8>, (Instant, Arc<Kit>)>>;

static STAGED: Mutex<StagedKits> = Mutex::new(None);

/// Uploads the kit's files to the phone, unless they were uploaded
/// recently for the same kit.  A failed staging is not cached, so the
/// next configuration apply tries again.
pub fn stage(mac: &[u8], ip: IpAddr, kit: &Arc<Kit>) {
    {
        let mut g = STAGED.lock().unwrap_or_else(|e| e.into_inner());
        let staged = g.get_or_insert_with(HashMap::new);
        if let Some((when, old)) = staged.get(mac) {
            if when.elapsed() < STAGE_TTL && Arc::ptr_eq(old, kit) {
                return;
            }
        }
        staged.insert(mac.to_vec(), (Instant::now(), kit.clone()));
    }
    dpma_log!(
        "Migration of {:#?}: staging the Open Desk Phone recovery and scripts on {}",
        String::from_utf8_lossy(mac),
        ip
    );
    let files = [
        (PAYLOAD_NAME, &kit.payload_script),
        (RECOVERY_NAME, &kit.recovery),
        (UPDATE_SCRIPT_NAME, &kit.update_script),
        (OTACERTS_NAME, &kit.otacerts),
    ];
    for (name, data) in files {
        if let Err(e) = upload(ip, name, data) {
            dpma_log!(
                "Migration of {:#?}: staging {} failed: {}",
                String::from_utf8_lossy(mac),
                name,
                e
            );
            STAGED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .map(|staged| staged.remove(mac));
            return;
        }
    }
}

/// Phones with a hand-off in progress, so that a re-applied configuration
/// does not start a second one.
static HANDING_OFF: Mutex<Option<HashSet<Vec<u8>>>> = Mutex::new(None);

/// Posts the image to the phone's stock updater, retrying while the phone
/// may still be running the payload: the configuration reaches the phone
/// within seconds, but the updater only accepts the image once the
/// re-keying is done.  Runs on its own thread because the hand-off takes
/// minutes; if the image is ultimately refused, the next configuration
/// apply handles it anyway.
pub fn hand_off(mac: Vec<u8>, ip: IpAddr, kit: Arc<Kit>) {
    if !HANDING_OFF
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(mac.clone())
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("odp-migrate".into())
        .spawn({
            let mac = mac.clone();
            move || {
                std::thread::sleep(FIRST_HAND_OFF_WAIT);
                for attempt in 1..=HAND_OFF_ATTEMPTS {
                    match push_eff(ip, &kit.eff) {
                        // The CGI answers 1 when `update-firmware.sh`
                        // exits with success, and a negative number
                        // otherwise.
                        Ok(1) => {
                            dpma_log!(
                                "Migration of {:#?}: the stock updater accepted Open Desk Phone {}; the phone restarts into its recovery",
                                String::from_utf8_lossy(&mac),
                                kit.version
                            );
                            break;
                        }
                        Ok(status) => {
                            dpma_log!(
                                "Migration of {:#?}: the stock updater refused the image (status {}){}",
                                String::from_utf8_lossy(&mac),
                                status,
                                if attempt == HAND_OFF_ATTEMPTS { "" } else { "; trying again" }
                            );
                        }
                        Err(e) => {
                            dpma_log!(
                                "Migration of {:#?}: handing the image to the stock updater failed: {}{}",
                                String::from_utf8_lossy(&mac),
                                e,
                                if attempt == HAND_OFF_ATTEMPTS { "" } else { "; trying again" }
                            );
                        }
                    }
                    if attempt == HAND_OFF_ATTEMPTS {
                        break;
                    }
                    std::thread::sleep(HAND_OFF_RETRY);
                }
                HANDING_OFF
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashSet::new)
                    .remove(&mac);
            }
        });
    if spawned.is_err() {
        // The thread could not be started; leave the hand-off to the
        // next configuration apply.
        HANDING_OFF
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashSet::new)
            .remove(&mac);
    }
}

// ---------------------------------------------------------------------------
// The phone's web server
// ---------------------------------------------------------------------------

/// Sends one HTTP POST to the phone and returns the full reply body.
fn post(
    ip: IpAddr,
    path: &str,
    content_type: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(ip, HTTP_PORT), CONNECT_TIMEOUT)
        .map_err(|e| format!("{}:{}: {e}", ip, HTTP_PORT))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| e.to_string())?;
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path,
        ip,
        content_type,
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|e| format!("{}:{}: {e}", ip, HTTP_PORT))?;
    // Read the reply headers, then the body: exactly `Content-Length`
    // bytes if given, otherwise everything until the connection closes.
    let mut head = Vec::with_capacity(0x400);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => head.push(byte[0]),
            Err(e) => return Err(format!("{}:{}: {e}", ip, HTTP_PORT)),
        }
        if head.len() > 0x4000 {
            return Err(format!("{}:{}: the reply headers never end", ip, HTTP_PORT));
        }
    }
    let text = String::from_utf8_lossy(&head);
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| format!("{}:{}: the reply is not HTTP: {:#?}", ip, HTTP_PORT, text))?;
    if code != 200 {
        return Err(format!("{}:{}: HTTP {}", ip, HTTP_PORT, code));
    }
    let mut length = None;
    for line in text.lines() {
        match line.split_once(':') {
            Some((name, value)) if name.trim().eq_ignore_ascii_case("content-length") => {
                length = value.trim().parse::<usize>().ok();
                break;
            }
            _ => {}
        }
    }
    let mut body = Vec::new();
    match length {
        Some(length) => {
            stream
                .take(length as u64)
                .read_to_end(&mut body)
                .map_err(|e| e.to_string())?;
        }
        None => {
            stream.read_to_end(&mut body).map_err(|e| e.to_string())?;
        }
    }
    Ok(body)
}

/// Builds the body of a `multipart/form-data` upload carrying `data`
/// under `name`, the way the phone's own upload form sends it.
fn multipart(boundary: &str, name: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(data.len() + 0x180);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// Uploads one file to the phone, saved under its own name in
/// `/data/digium/tmp`.
fn upload(ip: IpAddr, name: &str, data: &[u8]) -> Result<(), String> {
    let boundary = "ODP-MIGRATE-BOUNDARY";
    let reply = post(
        ip,
        UPLOAD_PATH,
        &format!("multipart/form-data; boundary={boundary}"),
        &multipart(boundary, name, data),
        UPLOAD_TIMEOUT,
    )?;
    // `<status>1</status>` is the uploader's success.
    if find(&reply, b"<status>1</status>").is_none() {
        return Err(format!(
            "the phone did not take the upload ({:?})",
            String::from_utf8_lossy(&reply)
        ));
    }
    Ok(())
}

/// Posts the image to the stock updater's CGI and returns the `<status>`
/// value from its reply.
fn push_eff(ip: IpAddr, eff: &[u8]) -> Result<i32, String> {
    // The CGI takes the image as the browser's upload form sends it: it
    // skips to the part's `application/octet-stream` header, and answers
    // -102 to a body without one.
    let boundary = "ODP-MIGRATE-BOUNDARY";
    let reply = post(
        ip,
        FIRMWARE_PATH,
        &format!("multipart/form-data; boundary={boundary}"),
        &multipart(boundary, "firmware.eff", eff),
        FIRMWARE_TIMEOUT,
    )?;
    // The reply looks like `<response>...<status>%d</status>...</response>`.
    let at = find(&reply, b"<status>").ok_or_else(|| {
        format!(
            "the reply names no status ({:?})",
            String::from_utf8_lossy(&reply)
        )
    })?;
    let rest = &reply[at + 8..];
    let end = rest
        .iter()
        .position(|&c| c == b'<')
        .ok_or_else(|| String::from("the reply's status is not terminated"))?;
    let status = std::str::from_utf8(&rest[..end])
        .map_err(|_| String::from("the reply's status is not text"))?
        .trim();
    status
        .parse()
        .map_err(|_| String::from("the reply's status is not a number"))
}

/// Finds the first occurrence of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn update_firmware_script() -> Vec<u8> {
    r#"#!/system/bin/sh
PATH=/system/bin
TMP=/data/digium/firmware
LOG='log -p v -t update-firmware.sh'

rm $TMP/update.zip;

$LOG "Updating firmware using $1";
if [ "PK" = "`busybox head -c 2 $1`" ]; then
  $LOG "Unencrypted, relocating to $TMP/update.zip";
  # use cp instead of mv to avoid cross-device link
  `cp $1 $TMP/update.zip 2>&1`;
  # Remove the original file to reduce the number of free bytes needed for a firmware update
  /system/bin/rm $1
else
  $LOG "Decrypting OTA update";
  busybox tail -c +101 $1 | openssl enc -d -aes-256-cbc -pass file:/system/etc/otapassphrase | busybox head -c -256 > $TMP/decrypted;

  # Remove the original file to reduce the number of free bytes needed for a firmware update
  /system/bin/rm $1

  # dump the third 100 bytes into the model number file
  busybox tail -c +201 $TMP/decrypted | busybox head -c 100 > $TMP/decrypted_model_number;

  if [ "`getprop ro.product.model`" != "`cat $TMP/decrypted_model_number`" ]; then
    # the model number doesn't match the model number of this device
    # best case, the recovery rejects it
    # worst case, this phone gets bricktastic
    # don't attempt the update
    $LOG "Device does not match firmware target"
    exit 1;
  fi

  # dump the remainder into the ota update file
  busybox tail -c +401 $TMP/decrypted > $TMP/update.zip;

  rm $TMP/decrypted
fi

# Where the Open Desk Phone recovery looks for a package: /cache, under
# the same name `dphone` uses.  The stock script leaves it in
# /data/digium/firmware, which the recovery does not mount.
$LOG "Flashing OTA update";
mkdir -p /cache/recovery
`cp $TMP/update.zip /cache/recovery/update.zip 2>&1`;
echo "boot-recovery" > /cache/recovery/command
echo "--update_package=/cache/recovery/update.zip" >> /cache/recovery/command
if [ "x${2}" = "xreset" ]; then
    echo "--wipe_data" >> /cache/recovery/command
fi
/sbin/uboot_env -m bootcmd='booti mmc3 recovery'
/system/bin/service call power 12 i32 0 s16 "firmware_update" i32 0 || reboot recovery
"#.into()
}

/// Builds the script that the `lan_port_mode` payload runs.  It is safe
/// to re-run: the server reruns it for as long as the phone is on stock
/// firmware, and each step either has already been done or aborts the
/// migration and leaves the phone unchanged.  (The stock updater refuses
/// the image until its key is ours, so a partial migration cannot install
/// anything.)
fn payload_script() -> Vec<u8> {
    format!(
        r#"#!/system/bin/sh
PATH=/system/bin:/sbin
TMP=/data/digium/tmp
LOG=$TMP/odp-migrate.log

echo "=== Open Desk Phone migration $(date) ===" >> $LOG

# Write our recovery over the stock one (partition 2, 8 MiB), and read
# it back to make sure it was written correctly.
dd if=$TMP/{RECOVERY_NAME} of=/dev/block/mmcblk0p2 || exit 1
sync
dd if=/dev/block/mmcblk0p2 of=$TMP/odp-recovery.readback bs=1048576 count=8 || exit 1
cmp $TMP/{RECOVERY_NAME} $TMP/odp-recovery.readback || {{
    echo "the recovery partition does not read back as written" >> $LOG
    rm $TMP/odp-recovery.readback
    exit 1
}}
rm $TMP/odp-recovery.readback

# Replace the updater script (which holds the updater's key) and the
# RecoverySystem trust store on /system.  The otapassphrase is left
# unchanged: Open Desk Phone images are encrypted with it, just like the
# stock ones.
mount -o remount,rw /system
cp $TMP/{UPDATE_SCRIPT_NAME} /system/bin/update-firmware.sh || exit 1
chmod 755 /system/bin/update-firmware.sh
cp $TMP/{OTACERTS_NAME} /system/etc/security/otacerts.zip
sync
mount -o remount,ro /system

# Restore lan_port_mode to a plain value so this script stops running on
# every configuration apply.  A concurrent apply may write the old value
# back; that is harmless, since the phone leaves stock firmware for good
# at the next restart.
CFG=`cat /data/digium/nvdata/config.xml`
echo "${{CFG//auto;sh \/data\/digium\/tmp\/odp-migrate.sh/auto}}" > /data/digium/nvdata/config.xml

rm $TMP/{RECOVERY_NAME} $TMP/{UPDATE_SCRIPT_NAME} $TMP/{OTACERTS_NAME}
echo "=== Open Desk Phone migration: the recovery and the keys are in place ===" >> $LOG
"#,
        RECOVERY_NAME = RECOVERY_NAME,
        UPDATE_SCRIPT_NAME = UPDATE_SCRIPT_NAME,
        OTACERTS_NAME = OTACERTS_NAME,
    )
    .into_bytes()
}
