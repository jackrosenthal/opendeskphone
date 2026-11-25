//! The updater against the HAL's simulated flash at the phone's real
//! geometry.

use opendeskphone_hal::Hal;
use opendeskphone_state::{Bank, Model};
use opendeskphone_update::{Updater, boot2, ctrl, inspect, trial};
use eff::ctrl::{CtrlRecord, Side};
use eff::EffHeader;

const BLOCK: usize = 128 * 1024;
const PAGE: usize = 2048;

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

fn kernel(seed: u32, len: usize) -> Vec<u8> {
    let mut k = prng(seed, len);
    k[0x24..0x28].copy_from_slice(&boot2::ZIMAGE_MAGIC.to_le_bytes());
    k[0x28..0x2c].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    k[0x2c..0x30].copy_from_slice(&(0x0200_0000u32 + len as u32).to_le_bytes());
    k
}

#[test]
fn install_on_the_hal_sim_at_real_geometry() {
    for model in [Model::D60, Model::D40] {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().expect("sim handle");
        let name = model.to_string();

        // Bank A: a running image; bank B: empty.
        let (ka, ra) = (kernel(1, 1_500_000), prng(2, 3 * BLOCK + 17));
        let ota_a = eff::ota::pack(&ka, &ra, "old", &format!("2_9_28_{name}_firmware.bin")).unwrap();
        sim.mtd_load("k0", &ka).unwrap();
        sim.mtd_load("root0", &ra).unwrap();
        let rec = CtrlRecord {
            active: 0,
            sides: [Side::from_header(&eff::ota::Ota::parse(&ota_a).unwrap().header.app), Side::INVALID],
        };
        let repl = opendeskphone_update::replication_for(model.family());
        ctrl::write(hal.mtd.open("ctrl").unwrap().as_mut(), &rec, repl).unwrap();

        // New image near the kernel slot's limit with two bad blocks in it.
        let kn = kernel(3, 2 * 1024 * 1024 - 2 * BLOCK - 4096);
        let mut rn = prng(4, 37 * BLOCK + 5000);
        rn[5 * BLOCK..7 * BLOCK].fill(0xff);
        let ota = eff::ota::pack(&kn, &rn, "new", &format!("3_0_0_{name}_firmware.bin")).unwrap();
        let eff = eff::envelope::pack(&EffHeader::new(&name, "3_0_0").unwrap(), &ota, [7; 8]).unwrap();
        sim.mtd_set_bad("k1", 0, true);
        sim.mtd_set_bad("k1", 9, true);
        sim.mtd_set_bad("root1", 2, true);
        sim.mtd_set_bad("ctrl", 2, true);

        let cmdline = "console=ttyAMA0,115200n8 root=/dev/mtdblock6 rootfstype=jffs2 ro";
        let done = Updater::new(hal.mtd.as_mut(), model, cmdline, tempfile::tempdir().unwrap().path()).install_eff(&eff[..]).unwrap();
        assert!(matches!(done.target, opendeskphone_update::Target::Bank { bank: Bank::B, previous: Bank::A, .. }), "{:?}", done.target);

        // Independent reading of the raw partition contents.
        let k1 = sim.mtd_contents("k1").unwrap();
        let mut got = Vec::new();
        for b in (0..16).filter(|b| *b != 0 && *b != 9) {
            got.extend_from_slice(&k1[b * BLOCK..(b + 1) * BLOCK]);
        }
        assert_eq!(&got[..kn.len()], &kn[..]);
        assert!(got[kn.len()..].iter().all(|b| *b == 0xff));
        let r1 = sim.mtd_contents("root1").unwrap();
        let mut got = Vec::new();
        for b in (0..r1.len() / BLOCK).filter(|b| *b != 2) {
            got.extend_from_slice(&r1[b * BLOCK..(b + 1) * BLOCK]);
        }
        assert_eq!(&got[..rn.len()], &rn[..]);
        assert!(got[rn.len()..].iter().all(|b| *b == 0xff));

        let c = sim.mtd_contents("ctrl").unwrap();
        let rec = CtrlRecord::parse(&c[..304]).unwrap();
        assert_eq!(rec.active, 1);
        assert_eq!(rec.sides[1].header().unwrap().name(), format!("3_0_0_{name}_firmware.bin"));
        let copies = (0..c.len() / PAGE).filter(|p| c[p * PAGE..p * PAGE + 304] == c[..304]).count();
        match model {
            Model::D60 => assert_eq!(copies, 3 * 64),
            _ => assert_eq!(copies, 3),
        }

        let boot = boot2::simulate(hal.mtd.as_mut()).unwrap();
        assert_eq!(boot.side, Bank::B);
        assert_eq!(boot2::load_kernel(hal.mtd.as_mut(), Bank::B).unwrap().unwrap(), kn);

        let status = inspect(hal.mtd.as_mut(), cmdline, None).unwrap();
        assert_eq!((status.running, status.next_boot), (Some(Bank::A), Bank::B));
        assert_eq!(status.banks[1].kernel_bad_blocks, 2);

        // And back.
        trial::rollback(hal.mtd.as_mut(), model, "root=/dev/mtdblock7", Bank::A).unwrap();
        assert_eq!(boot2::simulate(hal.mtd.as_mut()).unwrap().side, Bank::A);
    }
}
