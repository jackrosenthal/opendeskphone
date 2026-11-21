//! The screens of the touchscreen, which has no keys.
//!
//! - The list: touching a message opens its row, with "Play", "Call back"
//!   (green: it calls), and "Delete" (red) under it; touching the row
//!   again closes it, and touching another opens that one instead.  A new
//!   message has the voicemail icon.
//! - The message, while it plays and after: who left it and when; while it
//!   plays, the buttons that do what `*`, `0`, and `#` do on a phone with
//!   keys ("Rewind", "Pause", and "Skip"), and after, "Play"; "Call back";
//!   "Move" and "Mark unread"; and "Delete", across the bottom.
//! - Deleting asks first, on a screen with "Cancel" and "Delete".
//! - Without an application account the PBX cannot be asked for the
//!   messages: the app says so, and offers to call voicemail.

use super::*;
use opendeskphone_app_sdk::controls::{self, Button, GAP};
use opendeskphone_app_sdk::{Align, Font, Rect};

/// An icon that shows nothing (a picture the app does not define): read
/// messages have it, so that every row has the column of icons, and the
/// rows over the open one line up with those under it.
fn no_icon() -> Image {
    Image::Picture(String::new())
}

/// When a message was left, for its row: the time if it was today, else
/// the day.
fn when(date: u64) -> String {
    let offset = i64::from(l10n::utc_offset());
    let day = |t: i64| (t + offset).div_euclid(86_400);
    let today = day(date as i64) == day(timers::wall_time() as i64);
    l10n::format_time(date, if today { TimeStyle::Time } else { TimeStyle::Date })
}

/// The controls of the screen on the touchscreen.
#[derive(Default)]
pub(crate) struct Touched {
    /// Buttons, each with the softkey whose work it does.  On the list,
    /// the three under the open row.
    buttons: Vec<(Button, u32)>,
    /// The list's: the scroll widget, and the rows up to the open one and
    /// those after it.
    scroll: Widget,
    lists: (Widget, Widget),
    /// What the message's screen says the message is doing.
    status: Option<Widget>,
    pub(crate) confirm: Option<nav::Confirm>,
}

/// A line of text in `font`.
fn text(at: Rect, font: Font, line: &str) -> Widget {
    let w = ui::create(WidgetKind::Text, ROOT, at);
    ui::set_font(w, font);
    ui::set_align(w, Align::Left);
    ui::set_text(w, line);
    w
}

impl Voicemail {
    /// The open row: its message's place in the list.
    fn open_row(&self) -> Option<usize> {
        let id = self.expanded.as_ref()?;
        self.messages.iter().position(|m| m.id == *id)
    }

    pub(crate) fn draw_list_touch(&mut self) {
        self.touch = Touched::default();
        self.screen = Screen::List;
        let title = folder_title(&self.folder);
        let r = screen(&title);
        if self.messages.is_empty() {
            nav::note(r, &format!("There are no messages in {title}."));
            ui::set_softkeys(ROOT, &[Softkey::new(SK_FOLDERS, "Folders"), Softkey::new(SK_REFRESH, "Refresh")]);
            self.list = 0;
            return;
        }
        // The rows, and the buttons of the open one, scroll together: the
        // lists are as high as their rows, in a scroll widget.
        let m = ui::get_metrics();
        let scroll = ui::create(WidgetKind::Scroll, ROOT, r);
        let above = ui::create(WidgetKind::TextList, scroll, Rect::new(0, 0, m.width, m.row_height));
        let below = ui::create(WidgetKind::TextList, scroll, Rect::new(0, 0, m.width, m.row_height));
        let strip = Rect::new(GAP as i16, 0, m.width - 2 * GAP, m.row_height);
        let looks = [
            ("Play", Look::Primary, SK_PLAY),
            ("Call back", Look::Call, SK_CALL),
            ("Delete", Look::Destructive, SK_DELETE),
        ];
        for (place, (label, look, id)) in controls::columns(strip, 3).into_iter().zip(looks) {
            self.touch.buttons.push((Button::new(scroll, place, label, look), id));
        }
        self.touch.scroll = scroll;
        self.touch.lists = (above, below);
        self.list = above;
        ui::set_softkeys(ROOT, &[Softkey::new(SK_FOLDERS, "Folders")]);
        self.lay_out_list();
    }

    /// The list's rows as they now are, with the buttons under the open
    /// row.
    fn lay_out_list(&mut self) {
        let m = ui::get_metrics();
        let h = m.row_height;
        let n = self.messages.len();
        let open = self.open_row();
        let split = open.map_or(n, |i| i + 1);
        let items: Vec<ListItem> = self
            .messages
            .iter()
            .map(|msg| ListItem {
                text: msg.who().to_string(),
                detail: when(msg.date),
                icon: Some(if msg.read { no_icon() } else { Image::Icon(Icon::Voicemail) }),
            })
            .collect();
        let (above, below) = self.touch.lists;
        let top = split as u16 * h;
        ui::set_items(above, &items[..split]);
        ui::set_bounds(above, Rect::new(0, 0, m.width, top));
        let strip_h = if open.is_some() { h + GAP } else { 0 };
        let strip = Rect::new(GAP as i16, (top + GAP / 2) as i16, m.width - 2 * GAP, h);
        let number = open.is_some_and(|i| !self.messages[i].number.is_empty());
        for (place, (b, id)) in controls::columns(strip, 3).into_iter().zip(&mut self.touch.buttons) {
            b.place(place);
            b.set_visible(open.is_some());
            if *id == SK_CALL {
                b.set_enabled(number);
            }
        }
        ui::set_items(below, &items[split..]);
        ui::set_bounds(below, Rect::new(0, (top + strip_h) as i16, m.width, (n - split) as u16 * h));
        ui::set_visible(below, split < n);
        // No row is highlighted.  The buttons of an open row come into
        // view (the scroll widget follows the focus, once, when it moves).
        ui::set_focus(self.touch.scroll);
        if let (Some(_), Some((play, _))) = (open, self.touch.buttons.first()) {
            ui::set_focus(play.widget);
        }
    }

    /// A row of the list was touched: it opens, or it closes if it was
    /// open.
    pub(crate) fn row_touched(&mut self, w: Widget, row: u32) {
        let split = self.open_row().map_or(self.messages.len(), |i| i + 1);
        let i = match self.touch.lists {
            (above, _) if w == above => row as usize,
            (_, below) if w == below => split + row as usize,
            _ => return,
        };
        let Some(id) = self.messages.get(i).map(|m| m.id.clone()) else { return };
        self.expanded = if self.expanded.as_ref() == Some(&id) { None } else { Some(id) };
        self.lay_out_list();
    }

    /// The message's screen: while it plays, and after it has played.
    pub(crate) fn draw_message_touch(&mut self, msg: &Msg, playing: bool) {
        self.touch = Touched::default();
        let m = ui::get_metrics();
        let (h, line) = (m.row_height, m.line_height);
        let area = screen("Message");
        let (_, bottom) = controls::split_bottom(area);
        let (x, width) = (GAP as i16, m.width - 2 * GAP);
        let mut y = area.y + GAP as i16;
        let mut next = |height: u16| {
            let at = Rect::new(x, y, width, height);
            y += height as i16;
            at
        };
        // Who left it, and when, and how long it is.
        text(next(2 * line), Font::Large, msg.who());
        if !msg.name.is_empty() && !msg.number.is_empty() {
            text(next(line), Font::Normal, &msg.number);
        }
        let when = l10n::format_time(msg.date, TimeStyle::DateAndTime);
        text(next(line), Font::Normal, &format!("{when} \u{b7} {}:{:02}", msg.duration / 60, msg.duration % 60));
        self.touch.status = Some(text(next(line), Font::Normal, if playing { "Connecting\u{2026}" } else { "" }));

        let row = |y: &mut i16| {
            let at = Rect::new(x, *y + GAP as i16, width, h);
            *y += (h + GAP) as i16;
            at
        };
        let mut buttons = Vec::new();
        let first = row(&mut y);
        if playing {
            // What `*`, `0`, and `#` do on a phone that has them.
            let keys = [("Rewind", SK_REWIND), ("Pause", SK_PAUSE), ("Skip", SK_SKIP)];
            for (place, (label, id)) in controls::columns(first, 3).into_iter().zip(keys) {
                buttons.push((Button::new(ROOT, place, label, Look::Secondary), id));
            }
        } else {
            buttons.push((Button::new(ROOT, first, "Play", Look::Primary), SK_PLAY));
        }
        if !msg.number.is_empty() {
            buttons.push((Button::new(ROOT, row(&mut y), "Call back", Look::Call), SK_CALL));
        }
        let mark = if msg.read { "Mark unread" } else { "Mark read" };
        let pair = controls::columns(row(&mut y), 2);
        buttons.push((Button::new(ROOT, pair[0], "Move", Look::Secondary), SK_MOVE));
        buttons.push((Button::new(ROOT, pair[1], mark, Look::Secondary), SK_MARK));
        buttons.push((Button::new(ROOT, bottom, "Delete", Look::Destructive), SK_DELETE));
        self.touch.buttons = buttons;
        if playing {
            ui::claim_keys(ROOT, &[Key::Cancel, Key::Star, Key::Pound, Key::Digit(0)]);
        }
        self.screen = if playing { Screen::Playing } else { Screen::Message };
    }

    /// The playback call was answered: the message plays.
    pub(crate) fn playing_now(&mut self) {
        if let Some(status) = self.touch.status.filter(|_| self.screen == Screen::Playing) {
            ui::set_text(status, "Playing");
        }
    }

    /// Without an application account: what there is instead.
    pub(crate) fn show_no_account(&mut self) {
        self.touch = Touched::default();
        self.screen = Screen::NoAccount;
        let area = screen("Voicemail");
        let (rest, bottom) = controls::split_bottom(area);
        nav::note(rest, "Your messages cannot be listed on this phone.  Call voicemail to listen to them.");
        self.touch.buttons.push((Button::new(ROOT, bottom, "Call voicemail", Look::Call), SK_VOICEMAIL));
    }

    /// What a finger did on a widget that takes touches: the controls do
    /// what their softkeys do.
    pub(crate) fn touched(&mut self, w: Widget, t: &Touch) {
        let answer = self.touch.confirm.as_mut().and_then(|c| c.touched(w, t));
        if let Some(yes) = answer {
            return self.handle(Event::Softkey(if yes { SK_YES } else { SK_NO }));
        }
        let mut acted = None;
        for (b, id) in &mut self.touch.buttons {
            if b.touched(w, t) {
                acted = Some(*id);
            }
        }
        if let Some(id) = acted {
            self.handle(Event::Softkey(id));
        }
    }
}
