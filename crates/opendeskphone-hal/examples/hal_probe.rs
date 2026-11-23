//! Prints what the phone HAL backend finds on this machine.  Read-only
//! except with `--draw`, `--lamps`, and `--tone`; `--help` lists the
//! options.

use clap::Parser;
use opendeskphone_hal::*;
use std::time::Duration;

/// Print what the phone HAL backend finds on this machine
#[derive(Parser)]
struct Args {
    /// The model [default: the one on the kernel command line, else in the devicetree, else d60]
    #[arg(value_parser = |name: &str| name.parse::<Model>().map_err(|_| "not a model"))]
    model: Option<Model>,
    /// Draw a test pattern on every display
    #[arg(long)]
    draw: bool,
    /// Cycle the lamps
    #[arg(long)]
    lamps: bool,
    /// Play one second of 1 kHz on the handset
    #[arg(long)]
    tone: bool,
    /// Print key and hook events for this long
    #[arg(long, value_name = "SECONDS")]
    keys: Option<u64>,
}

fn main() {
    let args = Args::parse();
    let model = args
        .model
        .or_else(|| std::fs::read_to_string("/proc/cmdline").ok().and_then(|c| Model::from_cmdline(&c)))
        .or_else(|| {
            let compatible = std::fs::read("/proc/device-tree/compatible").ok()?;
            compatible.split(|b| *b == 0).filter_map(|s| std::str::from_utf8(s).ok()).find_map(Model::from_compatible)
        })
        .unwrap_or(Model::D60);
    println!("model: {model}");
    let mut hal = Hal::phone(model);
    for (name, err) in &hal.missing {
        println!("missing {name}: {err}");
    }

    for s in hal.display.screens() {
        let info = hal.display.info(s).unwrap();
        println!("display {s:?}: {}x{} stride {} {:?}", info.width, info.height, info.stride, info.format);
        if args.draw {
            let bpp = info.format.bytes_per_pixel();
            let fb = hal.display.frame(s).unwrap();
            for y in 0..info.height as usize {
                for x in 0..info.width as usize {
                    let v = (((x / 8) + (y / 8)) % 4 * 85) as u8;
                    let o = y * info.stride + x * bpp;
                    fb[o..o + bpp].fill(v);
                }
            }
            println!("  power on: {:?}", hal.display.set_power(s, true));
            println!("  flush: {:?}", hal.display.flush(s, &[]));
        }
    }
    for s in [Screen::Main, Screen::Side] {
        let max = hal.backlight.max_brightness(s);
        if max > 0 {
            println!(
                "backlight {s:?}: {:?}/{max}, contrast {:?}/{}",
                hal.backlight.brightness(s),
                hal.backlight.contrast(s),
                hal.backlight.max_contrast(s)
            );
        }
    }

    println!("lamps: {:?}", hal.lamps.lamps());
    if args.lamps {
        for lamp in hal.lamps.lamps() {
            for c in [LampColor::Red, LampColor::Green, LampColor::Amber, LampColor::Off] {
                let r = hal.lamps.set(lamp, c);
                if r.is_err() {
                    println!("  {lamp:?} {c:?}: {r:?}");
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }

    println!("off hook: {:?}", hal.input.off_hook());
    if let Some(secs) = args.keys {
        let end = std::time::Instant::now() + Duration::from_secs(secs);
        println!("press keys for {secs} s");
        while std::time::Instant::now() < end {
            if let Ok(Some(e)) = hal.input.next_event(Some(Duration::from_millis(200))) {
                println!("  {e:?}");
            }
        }
    }

    match hal.mtd.partitions() {
        Ok(parts) => {
            for p in parts {
                println!("mtd{} {:>8} {:#010x} erase {:#x}", p.index, p.name, p.size, p.erase_size);
            }
        }
        Err(e) => println!("mtd: {e}"),
    }

    println!("audio rate: {}", hal.audio.rate());
    match hal.audio.mixer() {
        Ok(mut m) => match m.controls() {
            Ok(ctls) => {
                for c in ctls {
                    let v = m.get(&c.name);
                    println!("  control {:?} {:?} {}..{} x{} = {:?} {:?}", c.name, c.kind, c.min, c.max, c.count, v, c.items);
                }
            }
            Err(e) => println!("mixer controls: {e}"),
        },
        Err(e) => println!("mixer: {e}"),
    }
    if args.tone {
        let rate = hal.audio.rate();
        let r = (|| -> Result<()> {
            let mut mixer = hal.audio.mixer()?;
            apply_board_gains(mixer.as_mut(), model, PcmDevice::Handset, 0)?;
            let mut cfg = PcmConfig::voice(model);
            cfg.capture = false;
            let mut pcm = hal.audio.open(PcmDevice::Handset, cfg)?;
            println!("  pcm config: {:?}", pcm.config());
            // 1 kHz square-ish wave from a small integer table.
            let period = (rate / 1000) as usize;
            let buf: Vec<i16> = (0..pcm.config().period_frames as usize)
                .map(|i| if (i % period) < period / 2 { 6000 } else { -6000 })
                .collect();
            for _ in 0..(rate / pcm.config().period_frames) {
                pcm.write(&buf)?;
            }
            Ok(())
        })();
        println!("tone: {r:?}");
    }

    let mut b = [0u8; 8];
    println!("rng: {:?} {:02x?}", hal.rng.fill(&mut b).is_ok(), b);
}
