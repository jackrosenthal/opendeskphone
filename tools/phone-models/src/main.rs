//! Builds the 3D models of the D40, D50, D60, D65, D70, and D80 that the
//! documentation's homepage turns: one binary glTF file per model, and
//! `models.json`, which lists the models in the order the page shows them.
//!
//!     cargo run --release -- [--out <dir>]
//!
//! The output directory is `docs/public/phones/` by default, which the
//! documentation site serves at `/phones/`.  Each model
//! carries the firmware's idle screen on the model's display, drawn by the
//! phone's own UI on the HAL simulator, and the key legends, drawn with
//! the repository's Roboto Bold.

mod geom;
mod gltf;
mod legends;
mod panel;
mod phone;
mod screens;

use clap::Parser;
use panel::{ModelId, Panel};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about = "Builds the 3D models of the phones for the documentation's homepage")]
struct Args {
    /// Where to write the models (default: the repository's docs/public/phones).
    #[arg(long)]
    out: Option<PathBuf>,
}

/// The model the page shows still, without animation.
const DEFAULT: ModelId = ModelId::D65;
/// The legends' pixels per panel unit.
const LEGEND_PX: f64 = 4.0;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn png(width: u32, height: u32, rgba: bool, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(if rgba { png::ColorType::Rgba } else { png::ColorType::Rgb });
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    enc.write_header().unwrap().write_image_data(pixels).unwrap();
    out
}

/// A gray display's picture as the LCD shows the picture: black is the
/// LCD's dark ink, white the LCD's lit ground.
fn lcd(p: &screens::Picture) -> Vec<u8> {
    const INK: [f64; 3] = [0x20 as f64, 0x26 as f64, 0x2b as f64];
    const GROUND: [f64; 3] = [0xc9 as f64, 0xd1 as f64, 0xcf as f64];
    p.rgb.as_chunks::<3>().0.iter().flat_map(|c| (0..3).map(move |i| (INK[i] + (GROUND[i] - INK[i]) * c[i] as f64 / 255.0).round() as u8)).collect()
}

fn screen_png(model: ModelId, p: &screens::Picture) -> Vec<u8> {
    let color = matches!(model, ModelId::D60 | ModelId::D65 | ModelId::D80);
    png(p.width, p.height, false, &if color { p.rgb.clone() } else { lcd(p) })
}

/// The legends' picture, cut down to the inked part; none for a panel
/// without legends.
fn legends_png(p: &Panel, fonts: &Path) -> Result<Option<phone::Print>, String> {
    if p.keys.iter().all(|k| k.legend.is_none()) {
        return Ok(None);
    }
    let pic = legends::rasterize(&legends::svg(p, LEGEND_PX, false), fonts)?;
    let (w, h) = (pic.width as usize, pic.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if pic.pixels[(y * w + x) * 4 + 3] > 0 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    let pad = 4;
    let (x0, y0, x1, y1) = (x0.saturating_sub(pad), y0.saturating_sub(pad), (x1 + pad).min(w), (y1 + pad).min(h));
    let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        px.extend_from_slice(&pic.pixels[(y * w + x0) * 4..(y * w + x1) * 4]);
    }
    let [ex0, ey0, _, _] = p.extent();
    let unit = |v: usize| v as f64 / LEGEND_PX;
    let bx = [ex0 + unit(x0), ey0 + unit(y0), ex0 + unit(x1), ey0 + unit(y1)];
    Ok(Some(phone::Print { png: png((x1 - x0) as u32, (y1 - y0) as u32, true, &px), at: bx }))
}

/// The page rocker's print over the rocker's own color, cut to the key's
/// box (the rocker's cap carries texture coordinates over that box).
fn rocker_png(p: &Panel, fonts: &Path) -> Result<Option<Vec<u8>>, String> {
    let Some(k) = p.keys.iter().find(|k| k.id == "page") else { return Ok(None) };
    let pic = legends::rasterize(&legends::svg(p, LEGEND_PX, true), fonts)?;
    let [ex0, ey0, _, _] = p.extent();
    let px = |v: f64| (v * LEGEND_PX).round() as usize;
    let (x0, y0) = (px(k.cx - k.w / 2.0 - ex0), px(k.cy - k.h / 2.0 - ey0));
    let (w, h) = (px(k.w), px(k.h));
    let silver = phone::srgb_bytes([0.66, 0.68, 0.70]);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = (y * pic.width as usize + x) * 4;
            let a = pic.pixels[i + 3] as f64 / 255.0;
            for (s, &ink) in silver.iter().zip(&pic.pixels[i..i + 3]) {
                out.push((*s as f64 * (1.0 - a) + ink as f64 * a).round() as u8);
            }
        }
    }
    Ok(Some(png(w as u32, h as u32, false, &out)))
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let out = args.out.unwrap_or_else(|| repo().join("docs/public/phones"));
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let fonts = repo().join("assets/fonts");
    let mut list = Vec::new();
    for model in ModelId::ALL {
        let p = panel::panel(model);
        let theme = match model {
            ModelId::D60 => "tides",
            ModelId::D65 => "harbor",
            ModelId::D80 => "aurora",
            _ => "",
        };
        let shots = screens::idle(model, theme);
        let tex = phone::Textures {
            legends: legends_png(&p, &fonts)?,
            rocker: rocker_png(&p, &fonts)?,
            screen: screen_png(model, &shots.main),
            side_screen: shots.side.as_ref().map(|s| screen_png(model, s)),
        };
        let m = phone::build(&p, tex);
        let glb = gltf::write(&m, model.name());
        let file = format!("{}.glb", model.name().to_lowercase());
        std::fs::write(out.join(&file), &glb.bytes).map_err(|e| format!("{file}: {e}"))?;
        let triangles: usize = m.parts.iter().map(|(_, mesh)| mesh.idx.len() / 3).sum();
        println!("{}: {} ({} KiB, {triangles} triangles), {:.0} mm across the turn, {:.0} mm high", model.name(), file, glb.bytes.len() / 1024, glb.radius * 2000.0, glb.height * 1000.0);
        let r = |v: f64| (v * 1e5).round() / 1e5;
        list.push(json!({"model": model.name(), "file": file, "box": glb.bounds.map(r)}));
    }
    let manifest = json!({"models": list, "still": DEFAULT.name()});
    std::fs::write(out.join("models.json"), serde_json::to_string_pretty(&manifest).unwrap() + "\n").map_err(|e| e.to_string())?;
    Ok(())
}
