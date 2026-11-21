//! Where BLF and rapid-dial items go: stock's assignment of configured
//! `<blf_item>`s to line keys and to the rapid-dial strip.
//!
//! The rules are those of stock's `phone` program, on the old family (D70)
//! and on the new family (D65):
//!
//! - There are `pages x line keys` line-key slots.  On every page the
//!   first N keys are the accounts, N being the number of accounts in use
//!   (at least one).
//! - `location="main"` names a line key by its absolute 0-based `index`;
//!   an index on an account key or past the last key is dropped.
//! - `location="side"` on the old family names a key of the rapid-dial
//!   strip by its 0-based `index` within a page (D50 and D70; the D40 and
//!   D45 have no strip).  On the new family, which has no strip, side items
//!   go onto the free line keys instead: their `index` is ignored, and a
//!   counter that starts at N, advances with every item of any location,
//!   and wraps back to N after the last key says which key.  Side items
//!   after the first main item are ignored.
//! - An item with `paging` (the default) goes on the first page where its
//!   key is free, so repeating a key fills page after page; on line keys
//!   only when line keys page (`page_linekeys`: the D65).  Otherwise it is
//!   written to its key on every page, replacing what was there.
//! - A blank item reserves its key.

use crate::config::{BlfConfig, BlfLocation};
use opendeskphone_state::{Family, Model};

/// What one key slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Slot {
    #[default]
    Empty,
    /// Reserved by a blank item.
    Blank,
    /// The item at this position of the configured list.
    Item(usize),
}

/// The assignment: `line[page][key]` and `side[page][key]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Line keys the accounts hold on every page.
    pub account_keys: u8,
    pub line: Vec<Vec<Slot>>,
    pub side: Vec<Vec<Slot>>,
    /// Pages in use: one past the highest page a paged item landed on.
    /// (An item on every page does not by itself make every page used;
    /// what stock counts there was not recovered.)
    pub pages: u8,
}

/// Puts `slot` on `key`; `pages` grows to cover the page a paged item
/// lands on.
fn place(grid: &mut [Vec<Slot>], key: usize, slot: Slot, on_first_free_page: bool, pages: &mut usize) {
    if on_first_free_page {
        if let Some(page) = grid.iter().position(|p| p[key] == Slot::Empty) {
            grid[page][key] = slot;
            *pages = (*pages).max(page + 1);
        }
    } else {
        for page in grid.iter_mut() {
            page[key] = slot;
        }
    }
}

/// Assigns `items` (in configuration order) to the keys of `model` with
/// `accounts` accounts in use.
pub fn layout(model: Model, accounts: usize, page_line_keys: bool, items: &[BlfConfig]) -> Layout {
    let keys = model.lines() as usize;
    let n = accounts.clamp(1, keys);
    let strip = model.blf_keys() as usize;
    let max_pages = model.blf_pages().max(1) as usize;
    let new_family = model.family() == Family::New;
    let mut line = vec![vec![Slot::Empty; keys]; max_pages];
    let mut side = vec![vec![Slot::Empty; strip]; max_pages];
    let mut counter = n;
    let mut seen_main = false;
    let mut pages = 1;
    for (i, item) in items.iter().enumerate() {
        let slot = if item.blank { Slot::Blank } else { Slot::Item(i) };
        let index = item.index as usize;
        match item.location {
            BlfLocation::Main => {
                seen_main = true;
                if (n..keys).contains(&index) {
                    place(&mut line, index, slot, item.paging && page_line_keys, &mut pages);
                }
            }
            BlfLocation::Side if new_family => {
                if !seen_main && counter < keys {
                    place(&mut line, counter, slot, item.paging && page_line_keys, &mut pages);
                }
            }
            BlfLocation::Expansion => {}
            BlfLocation::Side => {
                if index < strip {
                    place(&mut side, index, slot, item.paging, &mut pages);
                }
            }
        }
        if new_family {
            counter += 1;
            if counter >= keys {
                counter = n;
            }
        }
    }
    Layout { account_keys: n as u8, line, side, pages: pages as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_state::AccountId;

    fn item(location: BlfLocation, index: u8) -> BlfConfig {
        BlfConfig {
            id: format!("c{index}"),
            label: String::new(),
            number: "100".into(),
            subscribe: String::new(),
            account: AccountId(0),
            pickup_ringing: false,
            location,
            index,
            paging: true,
            blank: false,
        }
    }
    use BlfLocation::{Main, Side};
    use Slot::{Blank, Empty, Item};

    #[test]
    fn main_items_take_the_line_keys_the_accounts_leave() {
        // A D60 with one account: only key 1 is free.
        let items = [item(Main, 0), item(Main, 1), item(Main, 2)];
        let l = layout(Model::D60, 1, false, &items);
        assert_eq!((l.account_keys, l.pages), (1, 1));
        assert_eq!(l.line, [[Empty, Item(1)]]);
        // Two accounts: no key is free.  No account still keeps key 0.
        assert_eq!(layout(Model::D60, 2, false, &items).line, [[Empty, Empty]]);
        assert_eq!(layout(Model::D40, 0, false, &items).line, [[Empty, Item(1)]]);
        // Without line-key paging a repeated key is overwritten.
        let l = layout(Model::D62, 1, false, &[item(Main, 1), item(Main, 1)]);
        assert_eq!(l.line, [[Empty, Item(1)]]);
    }

    #[test]
    fn the_d65_pages_its_line_keys() {
        // Two accounts leave keys 2..5; repeating a key fills the next page.
        let mut items = vec![item(Main, 2), item(Main, 3), item(Main, 2), item(Main, 5), item(Main, 2)];
        items[3].paging = false;
        let l = layout(Model::D65, 2, true, &items);
        assert_eq!(l.pages, 3);
        assert_eq!(l.line[0], [Empty, Empty, Item(0), Item(1), Empty, Item(3)]);
        assert_eq!(l.line[1], [Empty, Empty, Item(2), Empty, Empty, Item(3)]);
        assert_eq!(l.line[2], [Empty, Empty, Item(4), Empty, Empty, Item(3)]);
        // The unpaged item is on every one of the twenty pages.
        assert!(l.line.len() == 20 && l.line[19][5] == Item(3));
        // With paging of line keys switched off, everything is on one page.
        let l = layout(Model::D65, 2, false, &items[..3]);
        assert_eq!((l.pages, l.line[0].clone()), (1, vec![Empty, Empty, Item(2), Item(1), Empty, Empty]));
    }

    #[test]
    fn side_items_on_the_new_family_fill_the_free_line_keys() {
        // D65, one account: five keys a page, in order, whatever the index.
        let items: Vec<BlfConfig> = (0..12).map(|i| item(Side, 40 + i)).collect();
        let l = layout(Model::D65, 1, true, &items);
        assert_eq!(l.pages, 3);
        assert_eq!(l.line[0], [Empty, Item(0), Item(1), Item(2), Item(3), Item(4)]);
        assert_eq!(l.line[1], [Empty, Item(5), Item(6), Item(7), Item(8), Item(9)]);
        assert_eq!(l.line[2], [Empty, Item(10), Item(11), Empty, Empty, Empty]);
        assert!(l.side.iter().all(|p| p.is_empty()));
        // D60: one free key, one page; the last side item wins it.
        let l = layout(Model::D60, 1, false, &items[..3]);
        assert_eq!(l.line, [[Empty, Item(2)]]);
        // The counter advances with main items too, and side items after
        // a main item are ignored.
        let items = [item(Side, 0), item(Main, 4), item(Side, 0)];
        let l = layout(Model::D65, 1, true, &items);
        assert_eq!(l.line[0], [Empty, Item(0), Empty, Empty, Item(1), Empty]);
        assert_eq!(l.pages, 1);
    }

    #[test]
    fn the_strip_of_the_old_family() {
        // D70: ten keys, ten pages; a repeated key goes to the next page,
        // an unpaged item to all of them.
        let mut items = vec![item(Side, 0), item(Side, 9), item(Side, 0), item(Side, 10), item(Main, 3), item(Side, 4)];
        items[5].blank = true;
        let l = layout(Model::D70, 2, false, &items);
        assert_eq!(l.pages, 2);
        assert_eq!(l.side[0], [Item(0), Empty, Empty, Empty, Blank, Empty, Empty, Empty, Empty, Item(1)]);
        assert_eq!(l.side[1][0], Item(2));
        assert_eq!(l.side[1][4], Empty);
        // The main item is on the line keys of every page.
        assert!(l.line.iter().all(|p| p[3] == Item(4)));
        items[2].paging = false;
        let l = layout(Model::D70, 2, false, &items);
        assert!(l.side.iter().all(|p| p[0] == Item(2)));
        // D50: one page; a second item on a used key is dropped.
        let l = layout(Model::D50, 1, false, &[item(Side, 3), item(Side, 3)]);
        assert_eq!((l.pages, l.side.len(), l.side[0][3]), (1, 1, Item(0)));
        // D40: no strip.
        let l = layout(Model::D40, 1, false, &[item(Side, 0)]);
        assert_eq!((l.line.clone(), l.side), (vec![vec![Empty, Empty]], vec![Vec::<Slot>::new()]));
    }
}
