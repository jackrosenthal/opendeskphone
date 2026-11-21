//! The configured BLF and rapid-dial items, as call control wants them:
//! each `<blf_item>` with its contact looked up in the contact lists on
//! disk.  Where the items go on the keys is `opendeskphone_sip::keys`.
//!
//! What follows stock's `phone` binary: an item needs `blank` or an id; a
//! contact that is in no contact list leaves a blank; with no items at all
//! and `enable_blf_on_unused_line_keys`, the contacts of the BLF contact
//! group (`blf_contact_group`) fill the free keys in order; a contact is
//! watched (one `presence` subscription) when it has `subscribe_to`, on the
//! first account, and at most `contacts_max_subscriptions` different
//! targets are watched; a contact with `can_pickup_ringing="1"` is picked
//! up, not dialed, by a press while it rings.
//!
//! Not done: application keys (`app_id`; the key is left reserved), the
//! old family's dataset autofill (`datasets.xml`), and provisioned
//! `<behaviors>` (lamp and press action by `target_status`): every item
//! has stock's default behavior.

use opendeskphone_config::contacts::{Contact, ContactGroup};
use opendeskphone_config::{Config, Nvdata};
use opendeskphone_sip::{BlfConfig, BlfLocation};
use opendeskphone_state::{AccountId, Family, Model};

/// Stock's default for `contacts_max_subscriptions`.
const MAX_SUBSCRIPTIONS: usize = 40;

/// The contact lists on disk.  A contact without an `id` gets one, as
/// stock's loader gives it (`_digiumContactsUniqueId%d`; the number here
/// counts such contacts across the lists): the contacts a DPMA server
/// generates for its lines ("Internal Extensions") have none, and each is
/// still its own contact with its own subscription.
fn contact_groups(nv: &Nvdata) -> Vec<ContactGroup> {
    let mut groups: Vec<ContactGroup> = Vec::new();
    for name in nv.contact_list_names().unwrap_or_default() {
        if let Ok(Some(bytes)) = nv.contact_list(&name)
            && let Ok(text) = String::from_utf8(bytes)
            && let Ok(found) = opendeskphone_config::contacts::parse_contacts(&text)
        {
            groups.extend(found);
        }
    }
    let unnamed = groups.iter_mut().flat_map(|g| &mut g.contacts).filter(|c| c.id.is_empty());
    for (n, c) in unnamed.enumerate() {
        c.id = format!("_digiumContactsUniqueId{n}");
    }
    groups
}

fn blank(location: BlfLocation, index: u8, paging: bool) -> BlfConfig {
    BlfConfig {
        id: String::new(),
        label: String::new(),
        number: String::new(),
        subscribe: String::new(),
        account: AccountId(0),
        pickup_ringing: false,
        location,
        index,
        paging,
        blank: true,
    }
}

fn of_contact(c: &Contact, account: AccountId, location: BlfLocation, index: u8, paging: bool) -> Option<BlfConfig> {
    let number = c.primary_number()?;
    let subscribe = c.subscribe_to.clone().unwrap_or_default();
    Some(BlfConfig {
        id: c.id.clone(),
        label: format!("{} {}", c.first_name, c.last_name).trim().to_string(),
        number: number.dial.clone(),
        subscribe,
        account,
        pickup_ringing: c.can_pickup_ringing,
        location,
        index,
        paging,
        blank: false,
    })
}

/// The contacts of the BLF contact group on the keys the accounts leave
/// free: per page the line keys, then (old family) the strip.
fn from_group(contacts: &[&Contact], model: Model, accounts: usize, page_line_keys: bool, account: AccountId) -> Vec<BlfConfig> {
    let keys = model.lines() as usize;
    let first = accounts.clamp(1, keys);
    let strip = if model.family() == Family::Old { model.blf_keys() as usize } else { 0 };
    let pages = model.blf_pages().max(1) as usize;
    let mut next = contacts.iter();
    let mut out = Vec::new();
    for page in 0..pages {
        if page == 0 || page_line_keys {
            for key in first..keys {
                let Some(c) = next.next() else { return out };
                out.extend(of_contact(c, account, BlfLocation::Main, key as u8, true));
            }
        }
        for key in 0..strip {
            let Some(c) = next.next() else { return out };
            out.extend(of_contact(c, account, BlfLocation::Side, key as u8, true));
        }
        if strip == 0 && !page_line_keys {
            break;
        }
    }
    out
}

/// The items for `model` with `accounts` accounts in use.
pub fn items(cfg: &Config, nv: &Nvdata, model: Model, accounts: usize, page_line_keys: bool, account: AccountId) -> Vec<BlfConfig> {
    // The local `/nvdata/blf_items.xml` when there is one, else the merged
    // configuration's items, as on stock.
    let configured = opendeskphone_config::ConfigStore::blf_items_from(nv, cfg);
    let groups = contact_groups(nv);
    let mut out: Vec<BlfConfig> = if configured.is_empty() {
        let group = cfg.setting("blf_contact_group").filter(|g| !g.is_empty());
        match group {
            Some(group) if cfg.setting_bool("enable_blf_on_unused_line_keys", true) => {
                let contacts: Vec<&Contact> =
                    groups.iter().filter(|g| g.id == group || g.group_name == group).flat_map(|g| &g.contacts).collect();
                from_group(&contacts, model, accounts, page_line_keys, account)
            }
            _ => Vec::new(),
        }
    } else {
        configured
            .iter()
            .filter_map(|i| {
                let location = match i.location.as_str() {
                    "main" => BlfLocation::Main,
                    "side" => BlfLocation::Side,
                    "expansion" => BlfLocation::Expansion,
                    _ => return None,
                };
                let index = i.index.min(255) as u8;
                let id = i.contact_id.as_deref().or(i.app_id.as_deref()).filter(|id| !id.is_empty());
                if !i.blank && id.is_none() {
                    return None;
                }
                let contact = match (&i.contact_id, i.blank) {
                    (Some(id), false) => groups.iter().flat_map(|g| &g.contacts).find(|c| &c.id == id),
                    _ => None,
                };
                Some(
                    contact
                        .and_then(|c| of_contact(c, account, location, index, i.paging))
                        .unwrap_or_else(|| blank(location, index, i.paging)),
                )
            })
            .collect()
    };
    // No more than the configured number of different targets is watched.
    let max = cfg
        .setting("contacts_max_subscriptions")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(MAX_SUBSCRIPTIONS);
    let mut watched: Vec<String> = Vec::new();
    for item in &mut out {
        if item.subscribe.is_empty() || watched.contains(&item.subscribe) {
            continue;
        }
        if watched.len() < max {
            watched.push(item.subscribe.clone());
        } else {
            item.subscribe.clear();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_config::xml::Element;

    const CONTACTS: &str = r#"<contacts group_name="Everyone" id="all">
        <contact id="101" first_name="Ann" last_name="Lee" contact_type="sip" subscribe_to="auto_hint_101">
          <numbers><number dial="101" label="Extension" primary="1"/></numbers></contact>
        <contact id="102" first_name="Bo" last_name="" contact_type="sip" subscribe_to="auto_hint_102" can_pickup_ringing="1">
          <numbers><number dial="102" label="Extension" primary="1"/></numbers></contact>
        <contact id="103" first_name="Cy" last_name="Ng" contact_type="sip">
          <numbers><number dial="5551234" label="Cell" primary="1"/></numbers></contact>
      </contacts>"#;

    fn setup(config: &str) -> (tempfile::TempDir, Nvdata, Config) {
        let dir = tempfile::tempdir().unwrap();
        let nv = Nvdata::new(dir.path());
        nv.write_contact_list("all.xml", CONTACTS.as_bytes()).unwrap();
        let root = Element::parse(config).unwrap();
        (dir, nv, Config::from_merged(root, opendeskphone_config::Model::D60, None))
    }

    #[test]
    fn items_keep_their_place_and_their_contacts() {
        let (_dir, nv, cfg) = setup(
            r#"<config><smart_blf><blf_items>
                 <blf_item location="side" index="3" contact_id="102"/>
                 <blf_item location="main" index="1" paging="0" contact_id="101"/>
                 <blf_item location="side" index="4" blank="1"/>
                 <blf_item location="side" index="5" contact_id="999"/>
                 <blf_item location="side" index="6" app_id="voicemail"/>
                 <blf_item location="side" index="7"/>
                 <blf_item location="expansion" index="0" contact_id="103"/>
               </blf_items></smart_blf></config>"#,
        );
        let got = items(&cfg, &nv, Model::D70, 1, false, AccountId(0));
        type Brief<'a> = (BlfLocation, u8, bool, bool, &'a str, &'a str, &'a str, bool);
        let brief: Vec<Brief> = got
            .iter()
            .map(|b| (b.location, b.index, b.paging, b.blank, b.id.as_str(), b.number.as_str(), b.subscribe.as_str(), b.pickup_ringing))
            .collect();
        use BlfLocation::{Expansion, Main, Side};
        assert_eq!(
            brief,
            [
                (Side, 3, true, false, "102", "102", "auto_hint_102", true),
                (Main, 1, false, false, "101", "101", "auto_hint_101", false),
                (Side, 4, true, true, "", "", "", false),
                // An unknown contact and an application key leave blanks;
                // an item with neither `blank` nor an id is no item.
                (Side, 5, true, true, "", "", "", false),
                (Side, 6, true, true, "", "", "", false),
                (Expansion, 0, true, false, "103", "5551234", "", false),
            ]
        );
        assert_eq!((got[0].label.as_str(), got[1].label.as_str()), ("Bo", "Ann Lee"));
    }

    /// The contacts a DPMA server makes of its lines carry no `id`.
    #[test]
    fn contacts_without_an_id_are_separate_contacts() {
        let dir = tempfile::tempdir().unwrap();
        let nv = Nvdata::new(dir.path());
        let list = r#"<contacts group_name="Internal Extensions" editable="0" >
            <contact first_name="Alice" last_name=" " contact_type="sip" account_id="1001" subscribe_to="auto_hint_1001">
            <numbers><number dial="1001" label="Extension" primary="1" /></numbers></contact>
            <contact first_name="Bob" last_name=" " contact_type="sip" account_id="1002" subscribe_to="auto_hint_1002">
            <numbers><number dial="1002" label="Extension" primary="1" /></numbers></contact>
            </contacts>"#;
        nv.write_contact_list("0", list.as_bytes()).unwrap();
        let root = Element::parse(r#"<config><setting id="blf_contact_group" value="Internal Extensions"/></config>"#).unwrap();
        let cfg = Config::from_merged(root, opendeskphone_config::Model::D65, None);
        let got = items(&cfg, &nv, Model::D65, 1, true, AccountId(0));
        let brief: Vec<(&str, &str, &str)> = got.iter().map(|b| (b.id.as_str(), b.label.as_str(), b.subscribe.as_str())).collect();
        assert_eq!(
            brief,
            [
                ("_digiumContactsUniqueId0", "Alice", "auto_hint_1001"),
                ("_digiumContactsUniqueId1", "Bob", "auto_hint_1002"),
            ]
        );
    }

    #[test]
    fn the_subscription_limit() {
        let (_dir, nv, cfg) = setup(
            r#"<config><setting id="contacts_max_subscriptions" value="1"/><smart_blf><blf_items>
                 <blf_item location="main" index="1" contact_id="101"/>
                 <blf_item location="main" index="2" contact_id="102"/>
                 <blf_item location="main" index="3" contact_id="101"/>
               </blf_items></smart_blf></config>"#,
        );
        let got = items(&cfg, &nv, Model::D65, 1, true, AccountId(0));
        let subs: Vec<&str> = got.iter().map(|b| b.subscribe.as_str()).collect();
        assert_eq!(subs, ["auto_hint_101", "", "auto_hint_101"]);
        assert_eq!(got[1].number, "102", "the second contact is still a rapid dial");
    }

    #[test]
    fn the_contact_group_fills_free_keys_when_nothing_is_configured() {
        let config = |extra: &str| format!(r#"<config><setting id="blf_contact_group" value="all"/>{extra}</config>"#);
        let (_dir, nv, cfg) = setup(&config(""));
        let place = |got: &[BlfConfig]| -> Vec<(BlfLocation, u8, String)> {
            got.iter().map(|b| (b.location, b.index, b.id.clone())).collect()
        };
        use BlfLocation::{Main, Side};
        // D60 with one account: one free line key.
        assert_eq!(place(&items(&cfg, &nv, Model::D60, 1, false, AccountId(0))), [(Main, 1, "101".to_string())]);
        // D65 with four accounts: two keys a page.
        assert_eq!(
            place(&items(&cfg, &nv, Model::D65, 4, true, AccountId(0))),
            [(Main, 4, "101".to_string()), (Main, 5, "102".to_string()), (Main, 4, "103".to_string())]
        );
        // D50 with all four accounts: the strip.
        assert_eq!(
            place(&items(&cfg, &nv, Model::D50, 4, false, AccountId(0))),
            [(Side, 0, "101".to_string()), (Side, 1, "102".to_string()), (Side, 2, "103".to_string())]
        );
        // Switched off, or no group named: nothing.
        let (_d, nv2, off) = setup(&config(r#"<setting id="enable_blf_on_unused_line_keys" value="0"/>"#));
        assert!(items(&off, &nv2, Model::D60, 1, false, AccountId(0)).is_empty());
        let (_d, nv3, none) = setup("<config/>");
        assert!(items(&none, &nv3, Model::D60, 1, false, AccountId(0)).is_empty());
    }
}
