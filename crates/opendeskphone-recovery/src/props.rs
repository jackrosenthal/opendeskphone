//! Android's system properties, as much as a package's `update-binary`
//! reads of them.
//!
//! The stock `update-binary` is an Android program, and a package's script
//! asks it for `ro.product.device` before it writes anything.  The program
//! reads properties from a file that Android's `init` shares with every
//! process, `/dev/__properties__`.  There is no Android `init` in this
//! recovery, so it writes that file itself, in the format of Android 4.4,
//! which is the stock recovery's and so the `update-binary`'s.
//!
//! The file is a header and then a tree: every node (`prop_bt`) has a part
//! of a name between two dots, its siblings to the left and right in
//! order (shorter names first, then by bytes), its children, and the
//! property (`prop_info`) of the name that ends at it.  The places are
//! byte offsets from the end of the header, and the tree's root is at 0.

/// The file's name.
pub const FILE: &str = "/dev/__properties__";
/// The file's length, as Android's `init` makes it.
const AREA: usize = 128 * 1024;
const HEADER: usize = 128;
const MAGIC: u32 = 0x504f_5250;
const VERSION: u32 = 0xfc6e_d0ab;
/// The longest value, with its zero.
const VALUE_MAX: usize = 92;

fn put(data: &mut [u8], at: usize, word: u32) {
    data[at..at + 4].copy_from_slice(&word.to_le_bytes());
}

fn get(data: &[u8], at: usize) -> usize {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize
}

/// A new node for `name`: its length, three unused bytes, the places of
/// its property, its left and right siblings, and its children, and then
/// the name with a zero.
fn node(data: &mut Vec<u8>, name: &[u8]) -> usize {
    let at = data.len();
    data.push(name.len() as u8);
    data.resize(at + 20, 0);
    data.extend_from_slice(name);
    data.push(0);
    data.resize(data.len().next_multiple_of(4), 0);
    at
}

/// The order of names among siblings.
fn before(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// The contents of the properties file for `properties`.
pub fn area(properties: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    // The root: a node with no name.
    let mut data = Vec::new();
    node(&mut data, b"");
    for (name, value) in properties {
        if name.is_empty() || name.len() > 31 || value.len() >= VALUE_MAX || name.split('.').any(str::is_empty) {
            return Err(format!("{name}: not a property that Android could hold"));
        }
        let mut current = 0;
        for part in name.split('.').map(str::as_bytes) {
            // The children of `current`: a tree in order, entered at the
            // first child made.
            let mut link = current + 16;
            loop {
                let at = get(&data, link);
                if at == 0 {
                    let new = node(&mut data, part);
                    put(&mut data, link, new as u32);
                    current = new;
                    break;
                }
                let len = data[at] as usize;
                match before(part, &data[at + 20..at + 20 + len]) {
                    std::cmp::Ordering::Equal => {
                        current = at;
                        break;
                    }
                    std::cmp::Ordering::Less => link = at + 8,
                    std::cmp::Ordering::Greater => link = at + 12,
                }
            }
        }
        // The property: a serial number with the value's length in its top
        // byte, the value in a field of fixed length, and the whole name.
        let info = data.len();
        data.extend_from_slice(&((value.len() as u32) << 24).to_le_bytes());
        data.extend_from_slice(value.as_bytes());
        data.resize(info + 4 + VALUE_MAX, 0);
        data.extend_from_slice(name.as_bytes());
        data.push(0);
        data.resize(data.len().next_multiple_of(4), 0);
        put(&mut data, current + 4, info as u32);
    }
    if HEADER + data.len() > AREA {
        return Err("the properties do not fit their file".into());
    }
    let mut out = vec![0u8; HEADER];
    put(&mut out, 0, data.len() as u32);
    put(&mut out, 8, MAGIC);
    put(&mut out, 12, VERSION);
    out.extend_from_slice(&data);
    out.resize(AREA, 0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A property's value, found the way Android 4.4's C library finds it
    /// (`find_property` and `__system_property_read` in
    /// `system_properties.c`).
    fn find(area: &[u8], name: &str) -> Option<String> {
        assert_eq!((get(area, 8) as u32, get(area, 12) as u32), (MAGIC, VERSION));
        let data = &area[HEADER..];
        let mut current = 0;
        for part in name.split('.').map(str::as_bytes) {
            let mut at = get(data, current + 16);
            loop {
                if at == 0 {
                    return None;
                }
                let len = data[at] as usize;
                match before(part, &data[at + 20..at + 20 + len]) {
                    std::cmp::Ordering::Equal => break,
                    std::cmp::Ordering::Less => at = get(data, at + 8),
                    std::cmp::Ordering::Greater => at = get(data, at + 12),
                }
            }
            current = at;
        }
        let info = get(data, current + 4);
        if info == 0 {
            return None;
        }
        let len = get(data, info) >> 24;
        let stored = &data[info + 4 + VALUE_MAX..];
        assert_eq!(&stored[..name.len() + 1], [name.as_bytes(), b"\0"].concat());
        Some(String::from_utf8(data[info + 4..info + 4 + len].to_vec()).unwrap())
    }

    #[test]
    fn properties_are_found_as_android_finds_them() {
        let all = [
            ("ro.product.device", "model80"),
            ("ro.product.model", "D80"),
            ("ro.build.display.id", "3_1_0"),
            ("ro.a", ""),
            ("sys.usb", "x"),
            ("ro", "top"),
        ];
        let area = area(&all).unwrap();
        assert_eq!(area.len(), AREA);
        for (name, value) in all {
            assert_eq!(find(&area, name).as_deref(), Some(value), "{name}");
        }
        for missing in ["ro.product", "ro.product.devic", "ro.product.device.x", "persist.x"] {
            assert_eq!(find(&area, missing), None, "{missing}");
        }
    }

    #[test]
    fn what_android_could_not_hold_is_refused() {
        assert!(area(&[("a..b", "x")]).is_err());
        assert!(area(&[("ro.x", &"v".repeat(92))]).is_err());
        assert!(area(&[(&"n".repeat(32), "x")]).is_err());
    }
}
