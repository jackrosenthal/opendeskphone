//! Contacts: the provisioned contact lists (the stock XML under
//! `/nvdata/contact_lists/`), by group, with search, presence, details,
//! and dialing.
//!
//! On the touchscreen, which has no keys, touching a contact shows its
//! details, and touching a number there dials it; there are no softkeys
//! for a highlighted row.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::xml;
use opendeskphone_app_sdk::{InputMode, LineState, Presence, PresenceStatus};

const SK_DIAL: u32 = 1;
const SK_DETAILS: u32 = 2;
const SK_SEARCH: u32 = 3;
const SK_GROUPS: u32 = 4;
const SK_EXIT: u32 = 5;
const SK_BACK: u32 = 6;
const SK_OPEN: u32 = 7;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out, and a touch opens a row.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_BACK, SK_OPEN, SK_GROUPS])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}

#[derive(Clone, Default)]
struct Number {
    dial: String,
    label: String,
    primary: bool,
}

#[derive(Clone, Default)]
struct Contact {
    name: String,
    organization: String,
    /// What presence is watched under (extension or hint), if anything.
    watch: String,
    numbers: Vec<Number>,
}

impl Contact {
    fn primary(&self) -> Option<&Number> {
        self.numbers
            .iter()
            .find(|n| n.primary)
            .or(self.numbers.first())
    }
}

#[derive(Clone, Default)]
struct Group {
    name: String,
    contacts: Vec<Contact>,
}

fn parse_group(el: &xml::Element) -> Group {
    let mut contacts: Vec<Contact> = el
        .children_named("contact")
        .map(|c| {
            let (first, last) = (c.attr("first_name"), c.attr("last_name"));
            let name = match (first.is_empty(), last.is_empty()) {
                (false, false) => format!("{first} {last}"),
                (false, true) => first.to_string(),
                (true, false) => last.to_string(),
                (true, true) => c.attr("organization").to_string(),
            };
            let mut nums = Vec::new();
            c.find_all("number", &mut nums);
            let numbers: Vec<Number> = nums
                .iter()
                .filter(|n| !n.attr("dial").is_empty())
                .map(|n| Number {
                    dial: n.attr("dial").to_string(),
                    label: n.attr("label").to_string(),
                    primary: n.attr("primary") == "1",
                })
                .collect();
            let watch = match c.attr("subscribe_to") {
                "" => String::new(),
                _ => numbers
                    .iter()
                    .find(|n| n.primary)
                    .or(numbers.first())
                    .map(|n| n.dial.clone())
                    .unwrap_or_default(),
            };
            Contact {
                name,
                organization: c.attr("organization").to_string(),
                watch,
                numbers,
            }
        })
        .filter(|c| !c.name.is_empty() || !c.numbers.is_empty())
        .collect();
    contacts.sort_by_key(|c| c.name.to_lowercase());
    let name = match el.attr("group_name") {
        "" | "null" => "Contacts".to_string(),
        n => n.to_string(),
    };
    Group { name, contacts }
}

/// Loads every group.  The phone's merged copy (`serialized_contacts.xml`)
/// is used only when no source list is present, so nothing shows twice.
fn load_groups() -> Vec<Group> {
    let lists = config::contact_lists();
    let mut groups = Vec::new();
    for pass in [false, true] {
        for (file, text) in &lists {
            if (file == "serialized_contacts.xml") != pass {
                continue;
            }
            let doc = xml::parse(text);
            let mut found = Vec::new();
            doc.find_all("contacts", &mut found);
            groups.extend(
                found
                    .into_iter()
                    .map(parse_group)
                    .filter(|g| !g.contacts.is_empty()),
            );
        }
        if !groups.is_empty() {
            break;
        }
    }
    groups
}

fn presence_text(p: &Presence) -> &'static str {
    use LineState as L;
    use PresenceStatus as S;
    match (p.line_state, p.status) {
        (L::Ringing, _) => "Ringing",
        (L::Busy, _) => "On call",
        (L::Hold, _) => "On hold",
        (_, S::Dnd) => "DND",
        (_, S::Away | S::Xa) => "Away",
        (_, S::Unavailable) | (L::Unavailable, _) => "Offline",
        (_, S::Available | S::Chat) | (L::Idle, _) => "Available",
        _ => "",
    }
}

/// Digits a name maps to on the keypad, so a numeric query finds names.
fn keypad_digits(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c.to_ascii_lowercase() {
            'a'..='c' => Some('2'),
            'd'..='f' => Some('3'),
            'g'..='i' => Some('4'),
            'j'..='l' => Some('5'),
            'm'..='o' => Some('6'),
            'p'..='s' => Some('7'),
            't'..='v' => Some('8'),
            'w'..='z' => Some('9'),
            d @ '0'..='9' => Some(d),
            _ => None,
        })
        .collect()
}

fn matches(c: &Contact, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    c.name.to_lowercase().contains(&q)
        || c.organization.to_lowercase().contains(&q)
        || c.numbers.iter().any(|n| n.dial.contains(&q))
        || (q.chars().all(|ch| ch.is_ascii_digit())
            && c.name
                .split_whitespace()
                .any(|w| keypad_digits(w).starts_with(&q)))
}

#[derive(Default)]
enum Screen {
    #[default]
    None,
    Groups(Widget),
    List(Widget),
    Details(Widget),
    Search {
        list: Widget,
    },
}

#[derive(Default)]
struct Contacts {
    groups: Vec<Group>,
    /// Group shown; `None` shows every contact.
    group: Option<usize>,
    /// Contacts on the current list or search result.
    view: Vec<Contact>,
    /// Contact on the detail screen.
    detail: Option<Contact>,
    query: String,
    screen: Screen,
    watched: Vec<String>,
}

impl Contacts {
    fn reload(&mut self) {
        self.groups = load_groups();
        for w in self.watched.drain(..) {
            presence::stop_observing(&w);
        }
        for c in self.groups.iter().flat_map(|g| &g.contacts) {
            if !c.watch.is_empty()
                && !self.watched.contains(&c.watch)
                && self.watched.len() < 200
                && presence::observe(&c.watch).is_ok()
            {
                self.watched.push(c.watch.clone());
            }
        }
        self.group = if self.groups.len() == 1 {
            Some(0)
        } else {
            None
        };
    }

    fn rows(view: &[Contact]) -> Vec<ListItem> {
        view.iter()
            .map(|c| {
                let state = match c.watch.as_str() {
                    "" => "",
                    w => presence::get(w).as_ref().map(presence_text).unwrap_or(""),
                };
                ListItem::with_detail(c.name.clone(), state)
            })
            .collect()
    }

    fn show_home(&mut self) {
        if self.groups.len() > 1 && self.group.is_none() {
            self.show_groups();
        } else {
            self.show_list();
        }
    }

    fn show_groups(&mut self) {
        self.group = None;
        let r = screen("Contacts");
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = self
            .groups
            .iter()
            .map(|g| ListItem::with_detail(g.name.clone(), g.contacts.len().to_string()))
            .collect();
        ui::set_items(list, &items);
        set_keys(&[
            Softkey::new(SK_OPEN, "Open"),
            Softkey::new(SK_SEARCH, "Search"),
            Softkey::new(SK_EXIT, "Exit"),
        ]);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.screen = Screen::Groups(list);
    }

    fn show_list(&mut self) {
        let (title, view) = match self.group.and_then(|i| self.groups.get(i)) {
            Some(g) => (g.name.clone(), g.contacts.clone()),
            None => (
                "Contacts".to_string(),
                self.groups
                    .iter()
                    .flat_map(|g| g.contacts.iter().cloned())
                    .collect(),
            ),
        };
        self.view = view;
        let r = screen(&title);
        if self.view.is_empty() {
            // (Touch: with what to do, in the upper part of the screen.)
            let text = if has_keys() {
                "There are no contacts."
            } else {
                "There are no contacts.  Your administrator can add them."
            };
            opendeskphone_app_sdk::nav::note(r, text);
            set_keys(&[Softkey::new(SK_EXIT, "Exit")]);
            self.screen = Screen::None;
            return;
        }
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        ui::set_items(list, &Self::rows(&self.view));
        let mut keys = Self::row_keys();
        keys.push(Softkey::new(SK_SEARCH, "Search"));
        if self.groups.len() > 1 {
            keys.push(Softkey::new(SK_GROUPS, "Groups"));
        }
        keys.push(Softkey::new(SK_EXIT, "Exit"));
        set_keys(&keys);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.screen = Screen::List(list);
    }

    /// The softkeys for the highlighted contact.  Touch: none; touching a
    /// contact shows its details.
    fn row_keys() -> Vec<Softkey> {
        if has_keys() {
            vec![
                Softkey::new(SK_DIAL, "Dial"),
                Softkey::new(SK_DETAILS, "Details"),
            ]
        } else {
            Vec::new()
        }
    }

    fn show_search(&mut self) {
        let r = screen("Search");
        let h = row_height();
        let input = ui::create(WidgetKind::Input, ROOT, Rect::new(2, r.y, r.width - 4, h));
        ui::set_input_mode(input, InputMode::Text);
        ui::set_value(input, &self.query);
        let list = ui::create(
            WidgetKind::TextList,
            ROOT,
            Rect::new(
                0,
                r.y + h as i16 + 2,
                r.width,
                r.height.saturating_sub(h + 2),
            ),
        );
        let mut keys = Self::row_keys();
        keys.push(Softkey::new(SK_BACK, "Back"));
        set_keys(&keys);
        ui::set_focus(input);
        self.screen = Screen::Search { list };
        self.filter();
    }

    fn filter(&mut self) {
        let Screen::Search { list } = self.screen else {
            return;
        };
        self.view = self
            .groups
            .iter()
            .flat_map(|g| g.contacts.iter())
            .filter(|c| matches(c, &self.query))
            .take(50)
            .cloned()
            .collect();
        ui::set_items(list, &Self::rows(&self.view));
    }

    fn selected(&self) -> Option<Contact> {
        let list = match self.screen {
            Screen::List(l) => l,
            Screen::Search { list } => list,
            _ => return None,
        };
        self.view.get(ui::selected(list)? as usize).cloned()
    }

    fn show_details(&mut self, c: Contact) {
        let r = screen(&c.name);
        let list = ui::create(WidgetKind::TextList, ROOT, r);
        let items: Vec<ListItem> = c
            .numbers
            .iter()
            .map(|n| {
                let label = if n.label.is_empty() {
                    "Number"
                } else {
                    &n.label
                };
                ListItem::with_detail(label.to_string(), n.dial.clone())
            })
            .collect();
        ui::set_items(list, &items);
        // Touch: touching a number dials it.
        let mut keys = vec![Softkey::new(SK_BACK, "Back")];
        if has_keys() {
            keys.insert(0, Softkey::new(SK_DIAL, "Dial"));
        }
        set_keys(&keys);
        opendeskphone_app_sdk::nav::focus_list(list);
        self.detail = Some(c);
        self.screen = Screen::Details(list);
    }

    fn dial(&mut self, number: &str) {
        if !number.is_empty() && calls::dial(number, None, &[]).is_ok() {
            runtime::background();
        }
    }

    fn dial_current(&mut self) {
        if let Screen::Details(list) = self.screen {
            let n = self
                .detail
                .as_ref()
                .and_then(|c| c.numbers.get(ui::selected(list)? as usize))
                .map(|n| n.dial.clone());
            if let Some(n) = n {
                self.dial(&n);
            }
        } else if let Some(c) = self.selected() {
            match c.numbers.len() {
                0 => {}
                1 => self.dial(&c.numbers[0].dial.clone()),
                _ => match c.primary().map(|n| n.dial.clone()) {
                    Some(n) => self.dial(&n),
                    None => self.show_details(c),
                },
            }
        }
    }

    fn back(&mut self) {
        match self.screen {
            Screen::Details(_) | Screen::Search { .. } => {
                self.query.clear();
                self.show_home();
            }
            Screen::List(_) if self.groups.len() > 1 => self.show_groups(),
            _ => runtime::background(),
        }
    }
}

impl App for Contacts {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                self.reload();
                self.query.clear();
                self.show_home();
            }
            Event::NewConfig => {
                self.reload();
                if runtime::in_foreground() {
                    self.show_home();
                }
            }
            Event::Presence(_) => {
                // Refresh the status column in place.
                if let Screen::List(l) | Screen::Search { list: l } = self.screen {
                    let keep = ui::selected(l);
                    ui::set_items(l, &Self::rows(&self.view));
                    if let Some(keep) = keep {
                        ui::set_selected(l, keep);
                    }
                }
            }
            Event::Widget((w, WidgetEvent::Activated(i))) => match self.screen {
                Screen::Groups(_) => {
                    self.group = Some(i as usize);
                    self.show_list();
                }
                Screen::Details(_) => self.dial_current(),
                Screen::List(l) | Screen::Search { list: l } if l == w && !has_keys() => {
                    if let Some(c) = self.view.get(i as usize).cloned() {
                        self.show_details(c);
                    }
                }
                Screen::List(l) | Screen::Search { list: l } if l == w => self.dial_current(),
                _ => {}
            },
            Event::Widget((_, WidgetEvent::Changed(text))) => {
                self.query = text;
                self.filter();
            }
            Event::Widget((_, WidgetEvent::Submitted(_))) => {
                if let Screen::Search { list } = self.screen
                    && !self.view.is_empty()
                {
                    opendeskphone_app_sdk::nav::focus_list(list);
                }
            }
            Event::Softkey(SK_OPEN) => {
                if let Screen::Groups(l) = self.screen
                    && let Some(row) = ui::selected(l)
                {
                    self.group = Some(row as usize);
                    self.show_list();
                }
            }
            Event::Softkey(SK_DIAL) => self.dial_current(),
            Event::Softkey(SK_DETAILS) => {
                if let Some(c) = self.selected() {
                    self.show_details(c);
                }
            }
            Event::Softkey(SK_SEARCH) => self.show_search(),
            Event::Softkey(SK_GROUPS) => self.show_groups(),
            Event::Softkey(SK_BACK) => self.back(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            Event::Key(Key::Cancel) => self.back(),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Contacts,
    name: "contacts",
    id: "org.opendeskphone.contacts",
    display_name: "Contacts",
    description: "Browse, search, and call your contacts",
    phone_icon: Some(Icon::AppContacts),
}
