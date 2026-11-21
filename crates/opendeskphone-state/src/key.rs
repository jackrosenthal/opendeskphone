//! Model-independent keys.  `opendeskphone-hal` maps evdev codes to these; the
//! UI, apps, and the web screen mirror all speak this vocabulary.

use serde::{Deserialize, Serialize};

/// A front-panel key.  The hookswitch is not a key; see
/// [`crate::Event::Hook`].
/// What a touch of the touchscreen (D80) did.  One finger is followed: a
/// touch begins with `Down`, moves with any number of `Move`s, and ends
/// with `Up` (the finger was lifted) or `Cancel` (the touch is to be
/// forgotten: nothing was touched).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    /// Dial pad `0`..`9` (the value is the digit).
    Digit(u8),
    Star,
    Pound,
    Up,
    Down,
    Left,
    Right,
    /// The check-mark key.
    Select,
    /// The X key.
    Cancel,
    VolumeUp,
    VolumeDown,
    /// Softkeys under the screen, 1..=4 from the left.
    Soft(u8),
    /// Line keys beside the screen, 1..=6 from the top.
    Line(u8),
    /// Dedicated BLF/rapid-dial keys, 1..=10 from the top (D50, D70).
    Blf(u8),
    Mute,
    Speaker,
    Headset,
    Hold,
    Transfer,
    Redial,
    /// Voicemail ("Msgs").
    Messages,
    /// Conference (D50, D70).
    Conference,
    /// Info (D50, D70).
    Info,
    /// Rapid-dial page left/right (D70), and the D65 page key (`BlfPageRight`).
    BlfPageLeft,
    BlfPageRight,
    /// D70 feature keys.
    Status,
    Contacts,
    Apps,
    Menu,
    /// Expansion-module key: module 0.., key index 0...
    Expansion { module: u8, key: u8 },
    /// A key whose function is not established; the value is the evdev code.
    Unknown(u16),
}

impl Key {
    /// The digit or symbol a dial-pad key enters.
    pub fn dial_char(self) -> Option<char> {
        match self {
            Key::Digit(d) if d <= 9 => Some((b'0' + d) as char),
            Key::Star => Some('*'),
            Key::Pound => Some('#'),
            _ => None,
        }
    }

    /// Dial-pad key for a character.
    pub fn from_dial_char(c: char) -> Option<Key> {
        match c {
            '0'..='9' => Some(Key::Digit(c as u8 - b'0')),
            '*' => Some(Key::Star),
            '#' => Some(Key::Pound),
            _ => None,
        }
    }

    /// The stock app-framework key name (`sk1`, `vup`, `line2`, and so on),
    /// used by the diagnostics and kept for app-API familiarity.
    pub fn name(self) -> String {
        match self {
            Key::Digit(d) => format!("{d}"),
            Key::Star => "*".into(),
            Key::Pound => "#".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Select => "select".into(),
            Key::Cancel => "cancel".into(),
            Key::VolumeUp => "vup".into(),
            Key::VolumeDown => "vdn".into(),
            Key::Soft(n) => format!("sk{n}"),
            Key::Line(n) => format!("line{n}"),
            Key::Blf(n) => format!("blf{n}"),
            Key::Mute => "mute".into(),
            Key::Speaker => "spkr".into(),
            Key::Headset => "hdst".into(),
            Key::Hold => "hold".into(),
            Key::Transfer => "xfer".into(),
            Key::Redial => "redial".into(),
            Key::Messages => "msgs".into(),
            Key::Conference => "conf".into(),
            Key::Info => "info".into(),
            Key::BlfPageLeft => "blfleft".into(),
            Key::BlfPageRight => "blfright".into(),
            Key::Status => "status".into(),
            Key::Contacts => "dir".into(),
            Key::Apps => "apps".into(),
            Key::Menu => "menu".into(),
            Key::Expansion { module, key } => format!("exp{module}.{key}"),
            Key::Unknown(c) => format!("key{c}"),
        }
    }

    /// Inverse of [`Key::name`].
    pub fn from_name(s: &str) -> Option<Key> {
        fn num(s: &str, prefix: &str, max: u8) -> Option<u8> {
            let n: u8 = s.strip_prefix(prefix)?.parse().ok()?;
            (1..=max).contains(&n).then_some(n)
        }
        Some(match s {
            "*" => Key::Star,
            "#" => Key::Pound,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "select" => Key::Select,
            "cancel" => Key::Cancel,
            "vup" => Key::VolumeUp,
            "vdn" => Key::VolumeDown,
            "mute" => Key::Mute,
            "spkr" => Key::Speaker,
            "hdst" => Key::Headset,
            "hold" => Key::Hold,
            "xfer" => Key::Transfer,
            "redial" => Key::Redial,
            "msgs" => Key::Messages,
            "conf" => Key::Conference,
            "info" => Key::Info,
            "blfleft" => Key::BlfPageLeft,
            "blfright" => Key::BlfPageRight,
            "status" => Key::Status,
            "dir" => Key::Contacts,
            "apps" => Key::Apps,
            "menu" => Key::Menu,
            _ => {
                if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() {
                    Key::Digit(s.as_bytes()[0] - b'0')
                } else if let Some(n) = num(s, "sk", 4) {
                    Key::Soft(n)
                } else if let Some(n) = num(s, "line", 6) {
                    Key::Line(n)
                } else if let Some(n) = num(s, "blf", 10) {
                    Key::Blf(n)
                } else if let Some(rest) = s.strip_prefix("exp") {
                    let (m, k) = rest.split_once('.')?;
                    Key::Expansion { module: m.parse().ok()?, key: k.parse().ok()? }
                } else {
                    Key::Unknown(s.strip_prefix("key")?.parse().ok()?)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let keys = [
            Key::Digit(0), Key::Digit(9), Key::Star, Key::Pound, Key::Up, Key::Select,
            Key::Cancel, Key::VolumeUp, Key::Soft(1), Key::Soft(4), Key::Line(6), Key::Blf(10),
            Key::Mute, Key::Speaker, Key::Headset, Key::Hold, Key::Transfer, Key::Redial,
            Key::Messages, Key::Conference, Key::Info, Key::BlfPageLeft, Key::BlfPageRight,
            Key::Status, Key::Contacts, Key::Apps, Key::Menu,
            Key::Expansion { module: 1, key: 17 }, Key::Unknown(700),
        ];
        for k in keys {
            assert_eq!(Key::from_name(&k.name()), Some(k), "{k:?}");
        }
        assert_eq!(Key::from_name("sk5"), None);
        assert_eq!(Key::Digit(5).dial_char(), Some('5'));
        assert_eq!(Key::from_dial_char('#'), Some(Key::Pound));
    }
}
