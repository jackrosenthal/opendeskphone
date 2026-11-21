//! The ringing alarm: its time, with Snooze and Dismiss.  The alarm clock
//! itself is the main role's (`opendeskphone_state::Alarms`); this screen comes
//! up when it rings and goes when it stops.
//!
//! On the touch skin (D80) Snooze and Dismiss are two large buttons under
//! the time, Snooze the larger (a hand reaching for a ringing alarm should
//! find it), and the title bar has no back button: leaving the screen is
//! not ending the alarm.

use super::*;

pub(crate) struct AlarmScreen;

const SK_SNOOZE: u32 = 1;
const SK_DISMISS: u32 = 4;

/// Touch: a button as large as these: `label` in the large font.
#[allow(clippy::too_many_arguments)]
fn large_button(c: &mut Canvas, th: &Theme, r: IRect, label: &str, fill: crate::gfx::Rgb, edge: Option<crate::gfx::Rgb>, fg: crate::gfx::Rgb, id: u32) {
    widgets::button(c, th, r, "", fill, edge, fg, Hit::Soft(id));
    c.text_in(th.font(Font::Large), r, label, fg, HAlign::Center);
}

impl Screen for AlarmScreen {
    fn name(&self) -> String {
        "alarm".into()
    }
    fn tag(&self) -> Tag {
        Tag::Alarm
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the buttons are on the screen itself.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(SK_SNOOZE, tr::SNOOZE), soft(0, ""), soft(0, ""), soft(SK_DISMISS, tr::DISMISS)]
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            SK_SNOOZE => cx.post(Command::SnoozeAlarm),
            SK_DISMISS => cx.post(Command::DismissAlarm),
            _ => {}
        }
        Nav::Stay
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            // Leaving the screen is not ending the alarm.
            Key::Cancel | Key::Select => {
                cx.post(Command::SnoozeAlarm);
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        boot::title_band(c, th, title, tr::ALARM);
        let Some(ringing) = &cx.st.alarms.ringing else { return };
        // Touch: the buttons at the bottom, and the time above them.
        let body = if th.touch() {
            let margin = th.px(16);
            let (rest, buttons) = body.split_bottom(th.touch_min * 4 + margin * 3);
            let buttons = buttons.inset_xy(margin, 0);
            let snooze = IRect::new(buttons.x, buttons.y, buttons.w, th.touch_min * 2 + margin);
            let dismiss = IRect::new(buttons.x, snooze.bottom() + margin, buttons.w, th.touch_min + margin);
            large_button(c, th, snooze, tr::SNOOZE, th.sel_bg, None, th.sel_fg, SK_SNOOZE);
            large_button(c, th, dismiss, tr::DISMISS, th.sk_bg, Some(th.sk_edge), th.sk_fg, SK_DISMISS);
            rest
        } else {
            body
        };
        let at = crate::time::Civil { hour: ringing.hour, minute: ringing.minute, ..cx.civil() };
        let h24 = cx.cfg.time_24h;
        let (time, mer) = (at.time(h24), at.meridiem(h24));
        let (big, sub) = (th.clock_font(), th.date_font());
        let gap = if th.gray { 4 } else { th.px(10) };
        let digits = big.glyph('0').map(|g| g.top as i32).unwrap_or(big.ascent as i32);
        let (tw, mw) = (big.width(&time), if mer.is_empty() { 0 } else { sub.width(mer) + gap / 2 });
        if th.touch() {
            // The alarm's icon above the time, the two in the middle.
            let icon = th.icon("app-alarms");
            let ih = icon.map(|i| i.height() + 2 * gap).unwrap_or(0);
            let top = body.y + (body.h - ih - digits) / 2;
            if let Some(icon) = icon {
                c.icon(icon, body.x + (body.w - icon.width()) / 2, top, th.fg);
            }
            let x = body.x + (body.w - tw - mw) / 2;
            let base = top + ih + digits;
            c.text(big, x, base, &time, th.fg);
            if !mer.is_empty() {
                c.text(sub, x + tw + gap / 2, base, mer, th.fg);
            }
            return;
        }
        let icon = th.icon("alarm");
        let iw = icon.map(|i| i.width() + gap).unwrap_or(0);
        let x = body.x + (body.w - iw - tw - mw) / 2;
        let base = body.y + (body.h + digits) / 2;
        if let Some(icon) = icon {
            c.icon(icon, x, base - (digits + icon.height()) / 2, th.fg);
        }
        c.text(big, x + iw, base, &time, th.fg);
        if !mer.is_empty() {
            c.text(sub, x + iw + tw + gap / 2, base, mer, th.fg);
        }
    }
}
