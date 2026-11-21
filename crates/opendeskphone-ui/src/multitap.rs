//! Multi-tap text entry, as on the stock firmware: repeated presses of a
//! digit key within a timeout cycle through its letters.
//!
//! The touch skin (D80), which has no keys, has the on-screen keyboard
//! ([`crate::osk`]) in its place; its screens still take digit keys from
//! elsewhere (the web mirror) this way.

use opendeskphone_state::Key;

/// How long after a press the next press of the same key still cycles.
pub const MULTITAP_TIMEOUT_MS: u64 = 1000;

fn cycle(key: Key) -> &'static str {
    match key {
        Key::Digit(1) => ".,@1-_/:;?!'\"()&%+=",
        Key::Digit(2) => "abc2ABC",
        Key::Digit(3) => "def3DEF",
        Key::Digit(4) => "ghi4GHI",
        Key::Digit(5) => "jkl5JKL",
        Key::Digit(6) => "mno6MNO",
        Key::Digit(7) => "pqrs7PQRS",
        Key::Digit(8) => "tuv8TUV",
        Key::Digit(9) => "wxyz9WXYZ",
        Key::Digit(0) => " 0",
        Key::Star => "*",
        Key::Pound => "#",
        _ => "",
    }
}

/// What a key press does to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Insert this character at the cursor.
    Insert(char),
    /// Replace the character before the cursor.
    Replace(char),
}

/// Multi-tap state for one input.
#[derive(Clone, Debug, Default)]
pub struct MultiTap {
    last: Option<Key>,
    index: usize,
    at_ms: u64,
}

impl MultiTap {
    /// Forgets the pending key (cursor moved, focus changed, timeout).
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// True while a further press of the same key would cycle.
    pub fn pending(&self, now_ms: u64) -> bool {
        self.last.is_some() && now_ms.saturating_sub(self.at_ms) < MULTITAP_TIMEOUT_MS
    }

    /// When the pending character commits, if one is pending.
    pub fn deadline_ms(&self) -> Option<u64> {
        self.last.map(|_| self.at_ms + MULTITAP_TIMEOUT_MS)
    }

    /// A dial-pad key was pressed at `now_ms`.
    pub fn tap(&mut self, key: Key, now_ms: u64) -> Option<Tap> {
        let letters = cycle(key);
        let n = letters.chars().count();
        if n == 0 {
            return None;
        }
        if self.last == Some(key) && self.pending(now_ms) && n > 1 {
            self.index = (self.index + 1) % n;
            self.at_ms = now_ms;
            return letters.chars().nth(self.index).map(Tap::Replace);
        }
        self.last = if n > 1 { Some(key) } else { None };
        self.index = 0;
        self.at_ms = now_ms;
        letters.chars().next().map(Tap::Insert)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_and_times_out() {
        let mut m = MultiTap::default();
        assert_eq!(m.tap(Key::Digit(2), 0), Some(Tap::Insert('a')));
        assert_eq!(m.tap(Key::Digit(2), 300), Some(Tap::Replace('b')));
        assert_eq!(m.tap(Key::Digit(2), 600), Some(Tap::Replace('c')));
        assert_eq!(m.tap(Key::Digit(2), 900), Some(Tap::Replace('2')));
        // Another key starts a new character.
        assert_eq!(m.tap(Key::Digit(3), 1000), Some(Tap::Insert('d')));
        // The same key after the timeout also starts a new character.
        assert_eq!(m.tap(Key::Digit(3), 2500), Some(Tap::Insert('d')));
        assert!(m.pending(2600));
        assert!(!m.pending(3600));
        // Wraps around.
        let mut m = MultiTap::default();
        m.tap(Key::Digit(0), 0);
        assert_eq!(m.tap(Key::Digit(0), 10), Some(Tap::Replace('0')));
        assert_eq!(m.tap(Key::Digit(0), 20), Some(Tap::Replace(' ')));
        // Single-character keys never cycle.
        assert_eq!(m.tap(Key::Star, 30), Some(Tap::Insert('*')));
        assert_eq!(m.tap(Key::Star, 40), Some(Tap::Insert('*')));
    }
}
