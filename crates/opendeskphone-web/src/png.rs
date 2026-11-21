//! A minimal PNG encoder for the screen mirror: 8-bit gray or 8-bit RGB,
//! no filtering, fast deflate, of a whole frame or a rectangle of it.
//! UI screens are mostly flat color, so a 522 KiB color frame of the
//! D60 shrinks to a few KiB.

use crate::{MirrorFormat, MirrorFrame};

/// The deflate level.  miniz's level 1 is a mode of its own that finds
/// few repeats: level 3 halves a picture of the D80's screen with a
/// wallpaper (a dithered one from 595 to 285 KiB, a flat one from 157 to
/// 80 KiB) for about twice the time (10 to 19 ms, and 5 to 9 ms, on a
/// workstation).  Level 6 gains another tenth at most for twice that
/// again; filtering gains nothing at level 3.
const LEVEL: u8 = 3;

/// A rectangle of a frame, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn area(&self) -> usize {
        self.w * self.h
    }
}

/// The bytes a pixel of `format` takes in a frame.
pub fn bytes_per_pixel(format: MirrorFormat) -> usize {
    match format {
        MirrorFormat::Xrgb8888 => 4,
        MirrorFormat::Gray8 => 1,
    }
}

/// Whether the frame's buffer is long enough for its stated geometry, and
/// the frame not empty.
pub fn sound(frame: &MirrorFrame) -> bool {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let bpp = bytes_per_pixel(frame.format);
    w > 0 && h > 0 && frame.stride >= w * bpp && frame.pixels.len() >= frame.stride * (h - 1) + w * bpp
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend(h.finalize().to_be_bytes());
}

/// Encodes a frame.  Returns `None` if the frame's buffer is too short for
/// its stated geometry.
pub fn encode(frame: &MirrorFrame) -> Option<Vec<u8>> {
    encode_rect(frame, Rect { x: 0, y: 0, w: frame.width as usize, h: frame.height as usize })
}

/// Encodes a rectangle of a frame.  Returns `None` if the frame's buffer
/// is too short for its stated geometry, or the rectangle is empty or not
/// within the frame.
pub fn encode_rect(frame: &MirrorFrame, r: Rect) -> Option<Vec<u8>> {
    let bpp_in = bytes_per_pixel(frame.format);
    let (bpp_out, color_type) = match frame.format {
        MirrorFormat::Xrgb8888 => (3, 2u8),
        MirrorFormat::Gray8 => (1, 0u8),
    };
    if !sound(frame) || r.area() == 0 || r.x + r.w > frame.width as usize || r.y + r.h > frame.height as usize {
        return None;
    }
    let (w, h) = (r.w, r.h);
    let mut raw = Vec::with_capacity(h * (1 + w * bpp_out));
    for row in r.y..r.y + h {
        raw.push(0); // filter: none
        let start = row * frame.stride + r.x * bpp_in;
        let line = &frame.pixels[start..start + w * bpp_in];
        match frame.format {
            MirrorFormat::Gray8 => raw.extend_from_slice(line),
            MirrorFormat::Xrgb8888 => {
                for px in line.as_chunks::<4>().0 {
                    raw.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(raw.len() / 8 + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, LEVEL));
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_idat(png: &[u8]) -> (u32, u32, u8, Vec<u8>) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut pos = 8;
        let (mut w, mut h, mut ct, mut data) = (0, 0, 0, Vec::new());
        while pos < png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &png[pos + 4..pos + 8];
            let body = &png[pos + 8..pos + 8 + len];
            let crc = u32::from_be_bytes(png[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            let mut hsh = crc32fast::Hasher::new();
            hsh.update(kind);
            hsh.update(body);
            assert_eq!(hsh.finalize(), crc);
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                    h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                    ct = body[9];
                }
                b"IDAT" => data = miniz_oxide::inflate::decompress_to_vec_zlib(body).unwrap(),
                _ => {}
            }
            pos += 12 + len;
        }
        (w, h, ct, data)
    }

    #[test]
    fn gray() {
        let f = MirrorFrame { width: 3, height: 2, stride: 4, format: MirrorFormat::Gray8, pixels: vec![1, 2, 3, 9, 4, 5, 6, 9] };
        let (w, h, ct, data) = decode_idat(&encode(&f).unwrap());
        assert_eq!((w, h, ct), (3, 2, 0));
        assert_eq!(data, [0, 1, 2, 3, 0, 4, 5, 6]);
    }

    #[test]
    fn color_swaps_to_rgb() {
        let f = MirrorFrame { width: 1, height: 1, stride: 4, format: MirrorFormat::Xrgb8888, pixels: vec![10, 20, 30, 0] };
        let (w, h, ct, data) = decode_idat(&encode(&f).unwrap());
        assert_eq!((w, h, ct), (1, 1, 2));
        assert_eq!(data, [0, 30, 20, 10]);
    }

    #[test]
    fn a_rectangle() {
        // 4 x 3, stride 5: the 2 x 2 at (1, 1).
        let f = MirrorFrame {
            width: 4,
            height: 3,
            stride: 5,
            format: MirrorFormat::Gray8,
            pixels: vec![0, 1, 2, 3, 9, 10, 11, 12, 13, 9, 20, 21, 22, 23, 9],
        };
        let (w, h, ct, data) = decode_idat(&encode_rect(&f, Rect { x: 1, y: 1, w: 2, h: 2 }).unwrap());
        assert_eq!((w, h, ct), (2, 2, 0));
        assert_eq!(data, [0, 11, 12, 0, 21, 22]);
        let f = MirrorFrame { width: 2, height: 1, stride: 8, format: MirrorFormat::Xrgb8888, pixels: vec![1, 2, 3, 0, 4, 5, 6, 0] };
        let (.., data) = decode_idat(&encode_rect(&f, Rect { x: 1, y: 0, w: 1, h: 1 }).unwrap());
        assert_eq!(data, [0, 6, 5, 4]);
        // Not within the frame, or empty.
        assert!(encode_rect(&f, Rect { x: 1, y: 0, w: 2, h: 1 }).is_none());
        assert!(encode_rect(&f, Rect { x: 0, y: 1, w: 1, h: 1 }).is_none());
        assert!(encode_rect(&f, Rect { x: 0, y: 0, w: 0, h: 1 }).is_none());
    }

    #[test]
    fn short_buffer() {
        let f = MirrorFrame { width: 4, height: 4, stride: 4, format: MirrorFormat::Gray8, pixels: vec![0; 8] };
        assert!(encode(&f).is_none());
    }
}
