//! Displays through DRM/KMS dumb buffers (no libdrm).
//!
//! Color models: the PL111 scans out of the dumb buffer directly.  Gray
//! models: the `digium-gray-lcd` driver pushes damage to the SPI panel on
//! `DIRTYFB`.  The D70 has two connector/CRTC pairs: 320x160 and 113x357.
//!
//! The D80's panel is mounted upside down: the first line of its
//! framebuffer is at the bottom of the screen as the phone is viewed, and
//! the kernel does not turn the picture.  So the frame the caller draws in
//! is a buffer of our own, the right way up, and a flush copies the damaged
//! parts into the framebuffer turned by 180 degrees.

use super::sys::*;
use crate::*;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeCardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DrmModeModeinfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    type_: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeGetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeGetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: DrmModeModeinfo,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeDestroyDumb {
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeFbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct DrmModeFbDirtyCmd {
    fb_id: u32,
    flags: u32,
    color: u32,
    num_clips: u32,
    clips_ptr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DrmClipRect {
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
}

pub(crate) const DRM_IOCTL_SET_MASTER: u32 = io(b'd', 0x1e);
pub(crate) const DRM_IOCTL_MODE_GETRESOURCES: u32 = iowr::<DrmModeCardRes>(b'd', 0xa0);
pub(crate) const DRM_IOCTL_MODE_SETCRTC: u32 = iowr::<DrmModeCrtc>(b'd', 0xa2);
pub(crate) const DRM_IOCTL_MODE_GETENCODER: u32 = iowr::<DrmModeGetEncoder>(b'd', 0xa6);
pub(crate) const DRM_IOCTL_MODE_GETCONNECTOR: u32 = iowr::<DrmModeGetConnector>(b'd', 0xa7);
pub(crate) const DRM_IOCTL_MODE_RMFB: u32 = iowr::<u32>(b'd', 0xaf);
pub(crate) const DRM_IOCTL_MODE_DIRTYFB: u32 = iowr::<DrmModeFbDirtyCmd>(b'd', 0xb1);
pub(crate) const DRM_IOCTL_MODE_CREATE_DUMB: u32 = iowr::<DrmModeCreateDumb>(b'd', 0xb2);
pub(crate) const DRM_IOCTL_MODE_MAP_DUMB: u32 = iowr::<DrmModeMapDumb>(b'd', 0xb3);
pub(crate) const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = iowr::<DrmModeDestroyDumb>(b'd', 0xb4);
pub(crate) const DRM_IOCTL_MODE_ADDFB2: u32 = iowr::<DrmModeFbCmd2>(b'd', 0xb8);

const DRM_MODE_CONNECTED: u32 = 1;
const DRM_MODE_FB_DIRTY_MAX_CLIPS: usize = 256;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}
pub(crate) const DRM_FORMAT_XRGB8888: u32 = fourcc(b'X', b'R', b'2', b'4');
pub(crate) const DRM_FORMAT_R8: u32 = fourcc(b'R', b'8', b' ', b' ');

fn drm_format(f: PixelFormat) -> (u32, u32) {
    match f {
        PixelFormat::Xrgb8888 => (DRM_FORMAT_XRGB8888, 32),
        PixelFormat::R8 => (DRM_FORMAT_R8, 8),
    }
}

struct DrmScreen {
    screen: Screen,
    info: FrameInfo,
    crtc_id: u32,
    connector_id: u32,
    fb_id: u32,
    handle: u32,
    map: *mut u8,
    map_len: usize,
    mode: DrmModeModeinfo,
    dirty_ok: bool,
    /// The CRTC shows this screen's framebuffer (`set_power`).
    on: bool,
    /// The frame the caller draws in, when the panel is upside down: the
    /// picture the right way up, `info.len()` bytes.  The framebuffer gets
    /// it turned, on a flush.
    upright: Option<Vec<u8>>,
    /// Bytes per row of the framebuffer (`info.stride` is the caller's
    /// frame's).
    fb_stride: usize,
}

/// Copies the part `r` of an upright picture (`src`, laid out as `info`
/// says) into a framebuffer of the same size and format whose picture is
/// turned by 180 degrees, and whose rows are `dst_stride` bytes: the pixel
/// at (x, y) goes to (width - 1 - x, height - 1 - y).
pub(crate) fn copy_turned(src: &[u8], info: &FrameInfo, dst: &mut [u8], dst_stride: usize, r: Rect) {
    let r = r.clip(info.width, info.height);
    let (w, h, bpp) = (info.width as usize, info.height as usize, info.format.bytes_per_pixel());
    for y in r.y as usize..(r.y + r.height) as usize {
        let from = &src[y * info.stride + r.x as usize * bpp..][..r.width as usize * bpp];
        // The row's last pixel is the turned row's first.
        let first = w - (r.x + r.width) as usize;
        let to = &mut dst[(h - 1 - y) * dst_stride + first * bpp..][..r.width as usize * bpp];
        for (d, p) in to.chunks_exact_mut(bpp).zip(from.chunks_exact(bpp).rev()) {
            d.copy_from_slice(p);
        }
    }
}

/// Where the part `r` of an upright picture is in the turned one.
pub(crate) fn turned_rect(r: Rect, width: u16, height: u16) -> Rect {
    let r = r.clip(width, height);
    Rect { x: width - r.x - r.width, y: height - r.y - r.height, ..r }
}

/// The phone's panels.  Opening them sets up the framebuffers and nothing
/// else: no mode is set, so a panel stays as the kernel left it (off) until
/// [`Display::set_power`], which shows the framebuffer as it is at that
/// moment.  The first picture a panel shows is therefore the caller's first
/// frame, not a cleared buffer.
pub struct DrmDisplay {
    file: File,
    screens: Vec<DrmScreen>,
}

// SAFETY: the mappings are owned by this object and only reached through
// `&self`/`&mut self`.
unsafe impl Send for DrmDisplay {}

fn ctx(what: &str) -> impl FnOnce(std::io::Error) -> HalError + '_ {
    move |e| HalError::io(format!("DRM {what}"), e)
}

impl DrmDisplay {
    /// Opens the first DRM card with a connected connector and sets up one
    /// dumb framebuffer per screen of `model`.
    pub fn open(model: Model) -> Result<DrmDisplay> {
        let mut last = HalError::Unavailable("no /dev/dri/card*".into());
        for n in 0..4 {
            let path = PathBuf::from(format!("/dev/dri/card{n}"));
            if !path.exists() {
                continue;
            }
            match Self::open_card(&path, model) {
                Ok(d) => return Ok(d),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn open_card(path: &std::path::Path, model: Model) -> Result<DrmDisplay> {
        let file = open_rw(path, 0).map_err(|e| HalError::io(path.display().to_string(), e))?;
        let fd = file.as_raw_fd();
        // First opener is master already; this only matters after a previous
        // master (e.g., a crashed main role) went away.
        let _ = ioctl_none(fd, DRM_IOCTL_SET_MASTER);

        let mut res = DrmModeCardRes::default();
        ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res).map_err(ctx("GETRESOURCES"))?;
        let mut crtcs = vec![0u32; res.count_crtcs as usize];
        let mut conns = vec![0u32; res.count_connectors as usize];
        let mut encs = vec![0u32; res.count_encoders as usize];
        let mut res2 = DrmModeCardRes {
            crtc_id_ptr: crtcs.as_mut_ptr() as usize as u64,
            connector_id_ptr: conns.as_mut_ptr() as usize as u64,
            encoder_id_ptr: encs.as_mut_ptr() as usize as u64,
            count_crtcs: crtcs.len() as u32,
            count_connectors: conns.len() as u32,
            count_encoders: encs.len() as u32,
            ..Default::default()
        };
        ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res2).map_err(ctx("GETRESOURCES"))?;

        let mut display = DrmDisplay { file, screens: Vec::new() };
        let mut used_crtcs: Vec<u32> = Vec::new();
        for &conn_id in &conns {
            // First call probes and returns counts; second fetches arrays.
            let mut c = DrmModeGetConnector { connector_id: conn_id, ..Default::default() };
            if ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c).is_err() {
                continue;
            }
            if c.count_modes == 0 || c.connection != DRM_MODE_CONNECTED && c.connection != 3 {
                continue;
            }
            let mut modes = vec![DrmModeModeinfo::default(); c.count_modes as usize];
            let mut cencs = vec![0u32; c.count_encoders as usize];
            let mut c2 = DrmModeGetConnector {
                connector_id: conn_id,
                modes_ptr: modes.as_mut_ptr() as usize as u64,
                count_modes: modes.len() as u32,
                encoders_ptr: cencs.as_mut_ptr() as usize as u64,
                count_encoders: cencs.len() as u32,
                ..Default::default()
            };
            ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c2).map_err(ctx("GETCONNECTOR"))?;
            let mode = modes[0];

            // Pick a CRTC: the current one, else the first free possible one.
            let mut crtc_id = 0;
            let mut candidates = Vec::new();
            if c2.encoder_id != 0 {
                candidates.push(c2.encoder_id);
            }
            candidates.extend(cencs.iter().copied());
            for enc_id in candidates {
                let mut e = DrmModeGetEncoder { encoder_id: enc_id, ..Default::default() };
                if ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &mut e).is_err() {
                    continue;
                }
                if e.crtc_id != 0 && !used_crtcs.contains(&e.crtc_id) {
                    crtc_id = e.crtc_id;
                    break;
                }
                if let Some(id) = crtcs
                    .iter()
                    .enumerate()
                    .find(|(i, id)| e.possible_crtcs & (1 << i) != 0 && !used_crtcs.contains(id))
                    .map(|(_, id)| *id)
                {
                    crtc_id = id;
                    break;
                }
            }
            if crtc_id == 0 {
                log::warn!("DRM connector {conn_id}: no usable CRTC");
                continue;
            }

            let size = Size::new(mode.hdisplay, mode.vdisplay);
            let screen = if Some(size) == model.side_screen() {
                Screen::Side
            } else if size == model.main_screen()
                || !display.screens.iter().any(|s| s.screen == Screen::Main)
            {
                Screen::Main
            } else {
                continue;
            };
            if display.screens.iter().any(|s| s.screen == screen) {
                continue;
            }
            if size != model.main_screen() && screen == Screen::Main {
                log::warn!(
                    "DRM main mode is {}x{}, expected {}x{} for {model}",
                    size.width, size.height, model.main_screen().width, model.main_screen().height
                );
            }
            used_crtcs.push(crtc_id);
            let s = display.setup_screen(screen, model, conn_id, crtc_id, mode)?;
            display.screens.push(s);
        }
        if display.screens.is_empty() {
            return Err(HalError::Unavailable(format!("{}: no connected display", path.display())));
        }
        Ok(display)
    }

    fn setup_screen(
        &mut self,
        screen: Screen,
        model: Model,
        connector_id: u32,
        crtc_id: u32,
        mode: DrmModeModeinfo,
    ) -> Result<DrmScreen> {
        let fd = self.file.as_raw_fd();
        let preferred = if model.is_color() { PixelFormat::Xrgb8888 } else { PixelFormat::R8 };
        let other = if model.is_color() { PixelFormat::R8 } else { PixelFormat::Xrgb8888 };
        let mut last_err = None;
        for format in [preferred, other] {
            let (four, bpp) = drm_format(format);
            let mut d = DrmModeCreateDumb {
                width: mode.hdisplay as u32,
                height: mode.vdisplay as u32,
                bpp,
                ..Default::default()
            };
            ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut d).map_err(ctx("CREATE_DUMB"))?;
            let mut fb = DrmModeFbCmd2 {
                width: d.width,
                height: d.height,
                pixel_format: four,
                ..Default::default()
            };
            fb.handles[0] = d.handle;
            fb.pitches[0] = d.pitch;
            if let Err(e) = ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &mut fb) {
                let mut dd = DrmModeDestroyDumb { handle: d.handle };
                let _ = ioctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &mut dd);
                last_err = Some(HalError::io(format!("DRM ADDFB2 {format:?}"), e));
                continue;
            }
            let mut m = DrmModeMapDumb { handle: d.handle, ..Default::default() };
            ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mut m).map_err(ctx("MAP_DUMB"))?;
            // SAFETY: mapping a dumb buffer we just created, for its full
            // size; musl's off_t is 64-bit so the fake offset fits.
            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    d.size as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    m.offset as libc::off_t,
                )
            };
            if map == libc::MAP_FAILED {
                return Err(HalError::last_os("DRM mmap"));
            }
            // The D80's panel is upside down (see the top of this file).
            let turned = model.is_touch() && screen == Screen::Main;
            let stride = if turned { mode.hdisplay as usize * format.bytes_per_pixel() } else { d.pitch as usize };
            let s = DrmScreen {
                screen,
                info: FrameInfo { width: mode.hdisplay, height: mode.vdisplay, stride, format },
                crtc_id,
                connector_id,
                fb_id: fb.fb_id,
                handle: d.handle,
                map: map as *mut u8,
                map_len: d.size as usize,
                mode,
                dirty_ok: true,
                on: false,
                upright: turned.then(|| vec![0; stride * mode.vdisplay as usize]),
                fb_stride: d.pitch as usize,
            };
            // SAFETY: `map` is `map_len` writable bytes.
            unsafe { std::ptr::write_bytes(s.map, 0, s.map_len) };
            return Ok(s);
        }
        Err(last_err.unwrap_or(HalError::Unavailable("no framebuffer format".into())))
    }

    fn set_crtc(&self, s: &DrmScreen, on: bool) -> Result<()> {
        let mut conn = s.connector_id;
        let mut c = DrmModeCrtc { crtc_id: s.crtc_id, ..Default::default() };
        if on {
            c.set_connectors_ptr = &mut conn as *mut u32 as usize as u64;
            c.count_connectors = 1;
            c.fb_id = s.fb_id;
            c.mode_valid = 1;
            c.mode = s.mode;
        }
        ioctl(self.file.as_raw_fd(), DRM_IOCTL_MODE_SETCRTC, &mut c).map_err(ctx("SETCRTC"))?;
        Ok(())
    }

    /// Brings an upside-down panel's framebuffer up to date with the
    /// caller's frame: the damaged parts, or all of it for no damage.
    fn turn_into_framebuffer(s: &DrmScreen, damage: &[Rect]) {
        let Some(upright) = &s.upright else { return };
        // SAFETY: the mapping is `map_len` writable bytes and lives as long
        // as the screen; nothing else holds a reference into it.
        let fb = unsafe { std::slice::from_raw_parts_mut(s.map, s.map_len) };
        let whole = [Rect::new(0, 0, s.info.width, s.info.height)];
        for r in if damage.is_empty() { &whole[..] } else { damage } {
            copy_turned(upright, &s.info, fb, s.fb_stride, *r);
        }
    }

    fn get(&self, screen: Screen) -> Result<&DrmScreen> {
        self.screens
            .iter()
            .find(|s| s.screen == screen)
            .ok_or(HalError::Unsupported("no such screen on this phone"))
    }
}

impl Display for DrmDisplay {
    fn screens(&self) -> Vec<Screen> {
        self.screens.iter().map(|s| s.screen).collect()
    }

    fn info(&self, screen: Screen) -> Option<FrameInfo> {
        self.get(screen).ok().map(|s| s.info)
    }

    fn frame(&mut self, screen: Screen) -> Result<&mut [u8]> {
        let s = self
            .screens
            .iter_mut()
            .find(|s| s.screen == screen)
            .ok_or(HalError::Unsupported("no such screen on this phone"))?;
        if let Some(upright) = s.upright.as_mut() {
            return Ok(upright);
        }
        // SAFETY: the mapping is at least `info.len()` bytes (pitch x
        // height) and lives as long as `self`; `&mut self` gives exclusive
        // access.
        Ok(unsafe { std::slice::from_raw_parts_mut(s.map, s.info.len().min(s.map_len)) })
    }

    fn flush(&mut self, screen: Screen, damage: &[Rect]) -> Result<()> {
        let fd = self.file.as_raw_fd();
        let s = self
            .screens
            .iter_mut()
            .find(|s| s.screen == screen)
            .ok_or(HalError::Unsupported("no such screen on this phone"))?;
        // A panel that is off shows nothing; turning it on shows the whole
        // buffer (the gray driver pushes it to the panel as it enables it).
        if !s.on {
            return Ok(());
        }
        Self::turn_into_framebuffer(s, damage);
        if !s.dirty_ok {
            return Ok(());
        }
        let turned = s.upright.is_some();
        let mut clips: Vec<DrmClipRect> = if damage.len() > DRM_MODE_FB_DIRTY_MAX_CLIPS {
            Vec::new()
        } else {
            damage
                .iter()
                .map(|r| if turned { turned_rect(*r, s.info.width, s.info.height) } else { r.clip(s.info.width, s.info.height) })
                .filter(|r| !r.is_empty())
                .map(|r| DrmClipRect { x1: r.x, y1: r.y, x2: r.x + r.width, y2: r.y + r.height })
                .collect()
        };
        if !damage.is_empty() && damage.len() <= DRM_MODE_FB_DIRTY_MAX_CLIPS && clips.is_empty() {
            return Ok(()); // everything clipped away
        }
        let mut cmd = DrmModeFbDirtyCmd {
            fb_id: s.fb_id,
            num_clips: clips.len() as u32,
            clips_ptr: if clips.is_empty() { 0 } else { clips.as_mut_ptr() as usize as u64 },
            ..Default::default()
        };
        match ioctl(fd, DRM_IOCTL_MODE_DIRTYFB, &mut cmd) {
            Ok(_) => Ok(()),
            // Scan-out framebuffers (the PL111) have no dirty callback.
            Err(e) if e.raw_os_error() == Some(libc::ENOSYS) => {
                s.dirty_ok = false;
                Ok(())
            }
            Err(e) => Err(HalError::io("DRM DIRTYFB", e)),
        }
    }

    fn capture(&self, screen: Screen) -> Result<Frame> {
        let s = self.get(screen)?;
        if let Some(upright) = &s.upright {
            return Ok(Frame { info: s.info, pixels: upright.clone() });
        }
        // SAFETY: as in `frame`; shared read of our own mapping.
        let pixels = unsafe { std::slice::from_raw_parts(s.map, s.info.len().min(s.map_len)) }.to_vec();
        Ok(Frame { info: s.info, pixels })
    }

    fn set_power(&mut self, screen: Screen, on: bool) -> Result<()> {
        let s = self.get(screen)?;
        if on {
            // The panel shows the frame as it is now.
            Self::turn_into_framebuffer(s, &[]);
        }
        self.set_crtc(s, on)?;
        if let Some(s) = self.screens.iter_mut().find(|s| s.screen == screen) {
            s.on = on;
        }
        Ok(())
    }
}

impl Drop for DrmDisplay {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        for s in &self.screens {
            // SAFETY: unmapping our own mapping exactly once.
            unsafe { libc::munmap(s.map as *mut libc::c_void, s.map_len) };
            let mut fb = s.fb_id;
            let _ = ioctl(fd, DRM_IOCTL_MODE_RMFB, &mut fb);
            let mut dd = DrmModeDestroyDumb { handle: s.handle };
            let _ = ioctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &mut dd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 x 3 picture of one byte a pixel, numbered by rows, in rows of
    /// five bytes; the framebuffer has rows of six.
    #[test]
    fn a_picture_is_turned_into_the_framebuffer() {
        let src: Vec<u8> = (0..3).flat_map(|y| [10 * y, 10 * y + 1, 10 * y + 2, 10 * y + 3, 99]).collect();
        let info = FrameInfo { width: 4, height: 3, stride: 5, format: PixelFormat::R8 };
        let mut fb = vec![0u8; 18];
        copy_turned(&src, &info, &mut fb, 6, Rect::new(0, 0, 4, 3));
        assert_eq!(fb, [23, 22, 21, 20, 0, 0, 13, 12, 11, 10, 0, 0, 3, 2, 1, 0, 0, 0]);
        // A part: the pixels (1, 0) and (2, 0) only, which are the two in
        // the middle of the last row.
        let mut fb = vec![0u8; 18];
        copy_turned(&src, &info, &mut fb, 6, Rect::new(1, 0, 2, 1));
        assert_eq!(fb, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0, 0]);
        assert_eq!(turned_rect(Rect::new(1, 0, 2, 1), 4, 3), Rect::new(1, 2, 2, 1));
        // What is outside the picture is left out.
        let mut fb = vec![0u8; 18];
        copy_turned(&src, &info, &mut fb, 6, Rect::new(3, 2, 10, 10));
        assert_eq!(fb, [23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// Four bytes a pixel stay in their order within the pixel.
    #[test]
    fn a_turned_pixel_keeps_its_bytes_in_order() {
        let src = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut fb = [0u8; 8];
        let info = FrameInfo { width: 2, height: 1, stride: 8, format: PixelFormat::Xrgb8888 };
        copy_turned(&src, &info, &mut fb, 8, Rect::new(0, 0, 2, 1));
        assert_eq!(fb, [5, 6, 7, 8, 1, 2, 3, 4]);
    }
}
