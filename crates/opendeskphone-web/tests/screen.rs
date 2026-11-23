//! The screen mirror (`ScreenService.WatchScreen`) end to end: the real
//! server on a loopback port with a screen whose pictures the test draws,
//! watched over raw HTTP/1.1 in the binary Connect framing the web page
//! uses.

use buffa::Message;
use opendeskphone_config::{ConfigStore, Nvdata};
use opendeskphone_state::{Audio, Model, Store};
use opendeskphone_web::{pb, MirrorFormat, MirrorFrame, MirrorScreen, ScreenSource, WebOptions, WebServer};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A screen whose picture the test draws.
struct Panel {
    frame: Mutex<MirrorFrame>,
    generation: AtomicU64,
}

impl Panel {
    fn new(width: u16, height: u16, format: MirrorFormat) -> Arc<Panel> {
        let bpp = if format == MirrorFormat::Gray8 { 1 } else { 4 };
        // Rows with padding at their ends, as a frame buffer may have.
        let stride = width as usize * bpp + 8;
        let mut pixels = vec![0u8; stride * height as usize];
        // Not flat: something for a picture to get wrong.
        for (i, p) in pixels.iter_mut().enumerate() {
            *p = (i % 251) as u8;
        }
        Arc::new(Panel { frame: Mutex::new(MirrorFrame { width, height, stride, format, pixels }), generation: AtomicU64::new(1) })
    }

    /// Changes the picture with `f`, and says it changed.
    fn draw(&self, f: impl FnOnce(&mut MirrorFrame)) {
        f(&mut self.frame.lock().unwrap());
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Fills a rectangle with one byte value.
    fn fill(&self, x: usize, y: usize, w: usize, h: usize, v: u8) {
        self.draw(|f| {
            let bpp = if f.format == MirrorFormat::Gray8 { 1 } else { 4 };
            for row in y..y + h {
                f.pixels[row * f.stride + x * bpp..row * f.stride + (x + w) * bpp].fill(v);
            }
        });
    }

    /// The picture as a PNG decoder gives it: rows of gray or RGB bytes.
    fn shown(&self) -> Vec<u8> {
        let f = self.frame.lock().unwrap();
        let mut out = Vec::new();
        for row in 0..f.height as usize {
            let line = &f.pixels[row * f.stride..];
            match f.format {
                MirrorFormat::Gray8 => out.extend_from_slice(&line[..f.width as usize]),
                MirrorFormat::Xrgb8888 => {
                    for px in line[..f.width as usize * 4].chunks(4) {
                        out.extend_from_slice(&[px[2], px[1], px[0]]);
                    }
                }
            }
        }
        out
    }
}

impl ScreenSource for Panel {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
    fn capture(&self, screen: MirrorScreen) -> Option<MirrorFrame> {
        (screen == MirrorScreen::Main).then(|| self.frame.lock().unwrap().clone())
    }
}

struct Phone {
    addr: SocketAddr,
    store: Store,
    panel: Arc<Panel>,
    dir: PathBuf,
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn phone(model: Model, panel: Arc<Panel>) -> Phone {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).unwrap();
    let dir = std::env::temp_dir().join(format!("opendeskphone-web-screen-{}", id.map(|b| format!("{b:02x}")).concat()));
    let nv = Nvdata::new(dir.join("nvdata"));
    nv.ensure_layout().unwrap();
    let config = Arc::new(Mutex::new(ConfigStore::open(nv, model.name().parse().unwrap(), dir.join("tmp")).unwrap()));
    let store = Store::new(model);
    let options = WebOptions { http: Some("127.0.0.1:0".parse().unwrap()), https: None, upload_dir: dir.join("upload"), ..Default::default() };
    let bound = WebServer::new(store.clone(), config, options).with_screen(panel.clone()).bind().await.unwrap();
    let addr = bound.http_addr().unwrap();
    tokio::spawn(bound.serve());
    Phone { addr, store, panel, dir }
}

/// Reads up to the end of an HTTP head; returns it and what came after.
async fn head(s: &mut TcpStream) -> (String, Vec<u8>) {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            return (String::from_utf8_lossy(&raw[..end]).into_owned(), raw[end + 4..].to_vec());
        }
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await.expect("a response").unwrap();
        assert!(n > 0, "closed: {:?}", String::from_utf8_lossy(&raw));
        raw.extend_from_slice(&buf[..n]);
    }
}

/// Logs in; returns the cookie and the CSRF token.
async fn login(addr: SocketAddr) -> (String, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let body = br#"{"user":"admin","password":"789"}"#;
    let req = format!(
        "POST /opendeskphone.v1.AuthService/Login HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let (head, mut rest) = head(&mut s).await;
    s.read_to_end(&mut rest).await.unwrap();
    let cookie = head.lines().find_map(|l| l.strip_prefix("set-cookie: ").or(l.strip_prefix("Set-Cookie: "))).expect("a cookie");
    let body = String::from_utf8_lossy(&rest);
    let csrf = body.split(r#""csrfToken":""#).nth(1).and_then(|t| t.split('"').next()).expect("a token");
    (cookie.split(';').next().unwrap().to_owned(), csrf.to_owned())
}

/// A `WatchScreen` stream, message by message.
struct Watch {
    s: TcpStream,
    /// Bytes of the body not yet taken apart (chunked).
    raw: Vec<u8>,
    /// The messages' bytes, unchunked, not yet decoded.
    data: Vec<u8>,
}

impl Watch {
    async fn open(p: &Phone, patches: bool) -> Watch {
        let (cookie, csrf) = login(p.addr).await;
        let msg = pb::WatchScreenRequest { screen: pb::ScreenId::Main.into(), max_fps: 15, patches }.encode_to_vec();
        let mut framed = vec![0u8];
        framed.extend((msg.len() as u32).to_be_bytes());
        framed.extend(&msg);
        let mut s = TcpStream::connect(p.addr).await.unwrap();
        let req = format!(
            "POST /opendeskphone.v1.ScreenService/WatchScreen HTTP/1.1\r\nHost: {}\r\nContent-Type: application/connect+proto\r\n\
             Cookie: {cookie}\r\nX-Csrf-Token: {csrf}\r\nContent-Length: {}\r\n\r\n",
            p.addr,
            framed.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        s.write_all(&framed).await.unwrap();
        let (head, raw) = head(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"), "{head}");
        Watch { s, raw, data: Vec::new() }
    }

    /// The next message, if one comes within `wait`.
    async fn within(&mut self, wait: Duration) -> Option<pb::WatchScreenResponse> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            // Whole chunks to data.
            while let Some(eol) = self.raw.windows(2).position(|w| w == b"\r\n") {
                let len = usize::from_str_radix(std::str::from_utf8(&self.raw[..eol]).unwrap().trim(), 16).unwrap();
                if self.raw.len() < eol + 2 + len + 2 {
                    break;
                }
                self.data.extend_from_slice(&self.raw[eol + 2..eol + 2 + len]);
                self.raw.drain(..eol + 2 + len + 2);
            }
            if self.data.len() >= 5 {
                assert_eq!(self.data[0], 0, "a message, not the end of the stream: {:?}", String::from_utf8_lossy(&self.data[5..]));
                let len = u32::from_be_bytes(self.data[1..5].try_into().unwrap()) as usize;
                if self.data.len() >= 5 + len {
                    let msg = pb::WatchScreenResponse::decode_from_slice(&self.data[5..5 + len]).unwrap();
                    self.data.drain(..5 + len);
                    return Some(msg);
                }
            }
            match tokio::time::timeout_at(deadline, self.s.read(&mut buf)).await {
                Err(_) => return None,
                Ok(n) => {
                    let n = n.unwrap();
                    assert!(n > 0, "the stream ended");
                    self.raw.extend_from_slice(&buf[..n]);
                }
            }
        }
    }

    async fn next(&mut self) -> pb::WatchScreenResponse {
        self.within(Duration::from_secs(10)).await.expect("a message")
    }
}

/// A PNG's size, its color type, and its pixels (gray or RGB rows).
fn decode(png: &[u8]) -> (u32, u32, u8, Vec<u8>) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let (mut w, mut h, mut ct, mut z) = (0, 0, 0, Vec::new());
    let mut pos = 8;
    while pos < png.len() {
        let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
        let body = &png[pos + 8..pos + 8 + len];
        match &png[pos + 4..pos + 8] {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                ct = body[9];
            }
            b"IDAT" => z.extend_from_slice(body),
            _ => {}
        }
        pos += 12 + len;
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap();
    let bpp = if ct == 0 { 1 } else { 3 };
    let line = 1 + w as usize * bpp;
    assert_eq!(raw.len(), line * h as usize);
    let mut px = Vec::new();
    for row in raw.chunks(line) {
        assert_eq!(row[0], 0, "no filter");
        px.extend_from_slice(&row[1..]);
    }
    (w, h, ct, px)
}

/// The picture a watcher has: the whole screen, with the patches drawn
/// over it as they come.
struct Mirrored {
    width: usize,
    bpp: usize,
    px: Vec<u8>,
}

impl Mirrored {
    fn whole(msg: &pb::WatchScreenResponse) -> Mirrored {
        assert!(!msg.png.is_empty(), "a whole picture");
        assert!(msg.patches.is_empty());
        let (w, h, ct, px) = decode(&msg.png);
        assert_eq!((w, h), (msg.width, msg.height));
        Mirrored { width: w as usize, bpp: if ct == 0 { 1 } else { 3 }, px }
    }

    fn apply(&mut self, msg: &pb::WatchScreenResponse) {
        assert!(msg.png.is_empty(), "patches, not a whole picture");
        for p in msg.patches.iter() {
            let (w, h, _, px) = decode(&p.png);
            assert_eq!((w, h), (p.width, p.height));
            for row in 0..h as usize {
                let at = ((p.y as usize + row) * self.width + p.x as usize) * self.bpp;
                let n = w as usize * self.bpp;
                self.px[at..at + n].copy_from_slice(&px[row * n..(row + 1) * n]);
            }
        }
    }
}

fn rects(msg: &pb::WatchScreenResponse) -> Vec<(u32, u32, u32, u32)> {
    msg.patches.iter().map(|p| (p.x, p.y, p.width, p.height)).collect()
}

#[tokio::test]
async fn a_watcher_that_takes_patches_gets_the_whole_screen_and_then_what_changed() {
    let p = phone(Model::D80, Panel::new(800, 1280, MirrorFormat::Xrgb8888)).await;
    let mut w = Watch::open(&p, true).await;
    let first = w.next().await;
    assert_eq!((first.width, first.height), (800, 1280));
    let mut copy = Mirrored::whole(&first);
    assert_eq!(copy.px, p.panel.shown());

    // A call's timer at the bottom: that rectangle alone.
    p.panel.fill(700, 1200, 30, 12, 0xee);
    let msg = w.next().await;
    assert_eq!(rects(&msg), [(700, 1200, 30, 12)]);
    assert_eq!((msg.width, msg.height), (800, 1280));
    copy.apply(&msg);
    assert_eq!(copy.px, p.panel.shown());

    // Two changes far apart: two patches, and the copy is the screen.
    p.panel.draw(|f| {
        f.pixels[5 * f.stride + 7 * 4] ^= 1;
        f.pixels[900 * f.stride + 600 * 4 + 2] ^= 1;
    });
    let msg = w.next().await;
    assert_eq!(rects(&msg), [(7, 5, 1, 1), (600, 900, 1, 1)]);
    copy.apply(&msg);
    assert_eq!(copy.px, p.panel.shown());

    // A picture shown again as it was is nothing to send: the next
    // message is the handset's.
    p.panel.draw(|_| {});
    tokio::time::sleep(Duration::from_millis(300)).await;
    p.store.update::<Audio, _>(|a| a.off_hook = true);
    let msg = w.next().await;
    assert!(msg.off_hook);
    assert!(msg.png.is_empty() && msg.patches.is_empty(), "{:?}", rects(&msg));

    // Most of the screen: the whole of it again.
    p.panel.fill(0, 0, 800, 700, 0x40);
    let msg = w.next().await;
    let copy = Mirrored::whole(&msg);
    assert_eq!(copy.px, p.panel.shown());

    // A watcher that comes now begins with the whole screen, as it is.
    let mut late = Watch::open(&p, true).await;
    assert_eq!(Mirrored::whole(&late.next().await).px, p.panel.shown());
}

#[tokio::test]
async fn a_watcher_that_does_not_ask_for_patches_gets_whole_pictures() {
    // A gray screen, as the D70's side display or a D40's.
    let p = phone(Model::D40, Panel::new(160, 90, MirrorFormat::Gray8)).await;
    let mut w = Watch::open(&p, false).await;
    let first = Mirrored::whole(&w.next().await);
    assert_eq!(first.bpp, 1);
    assert_eq!(first.px, p.panel.shown());
    p.panel.fill(3, 4, 5, 6, 0);
    let msg = w.next().await;
    assert_eq!(Mirrored::whole(&msg).px, p.panel.shown());

    // The same screen, patches asked for: a gray patch.
    let mut w = Watch::open(&p, true).await;
    let mut copy = Mirrored::whole(&w.next().await);
    p.panel.fill(150, 80, 10, 10, 9);
    let msg = w.next().await;
    assert_eq!(rects(&msg), [(150, 80, 10, 10)]);
    copy.apply(&msg);
    assert_eq!(copy.px, p.panel.shown());
}

#[tokio::test]
async fn patches_drawn_in_order_keep_the_copy_the_screen() {
    let p = phone(Model::D60, Panel::new(480, 272, MirrorFormat::Xrgb8888)).await;
    let mut w = Watch::open(&p, true).await;
    let mut copy = Mirrored::whole(&w.next().await);
    // Changes over one another, some of them faster than the watcher's
    // rate (which sends what the screen is when it looks).
    let mut seed = 7u32;
    let mut rand = |n: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 8) % n
    };
    for round in 0..40 {
        let (x, y) = (rand(470) as usize, rand(262) as usize);
        let (cw, ch) = (1 + rand(480 - x as u32) as usize, 1 + rand(272 - y as u32) as usize);
        p.panel.fill(x, y, cw.min(60), ch.min(40), rand(256) as u8);
        if round % 3 == 0 {
            continue;
        }
        let msg = w.next().await;
        if msg.png.is_empty() {
            copy.apply(&msg);
        } else {
            copy = Mirrored::whole(&msg);
        }
        // The copy is the screen once the watcher has caught up.
        if copy.px != p.panel.shown() {
            let msg = w.next().await;
            copy.apply(&msg);
        }
        assert_eq!(copy.px, p.panel.shown(), "round {round}");
    }
}
