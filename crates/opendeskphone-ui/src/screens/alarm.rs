//! The ringing alarm: its time, with Snooze and Dismiss.  The alarm clock
//! itself is the main role's (`opendeskphone_state::Alarms`); this screen comes
//! up when it rings and goes when it stops.

use super::*;

pub(crate) struct AlarmScreen;

const SK_SNOOZE: u32 = 1;
const SK_DISMISS: u32 = 4;

impl Screen for AlarmScreen {
    fn name(&self) -> String {
        "alarm".into()
    }
    fn tag(&self) -> Tag {
        Tag::Alarm
    }
    fn softkeys(&self, _cx: &Cx) -> Vec<Soft> {
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
        widgets::title_bar(c, th, title, tr::ALARM);
        let Some(ringing) = &cx.st.alarms.ringing else { return };
        let at = crate::time::Civil { hour: ringing.hour, minute: ringing.minute, ..cx.civil() };
        let h24 = cx.cfg.time_24h;
        let (time, mer) = (at.time(h24), at.meridiem(h24));
        let (big, sub) = (th.clock_font(), th.date_font());
        let icon = th.icon("alarm");
        let gap = if th.gray { 4 } else { th.px(10) };
        let digits = big.glyph('0').map(|g| g.top as i32).unwrap_or(big.ascent as i32);
        let (tw, mw) = (big.width(&time), if mer.is_empty() { 0 } else { sub.width(mer) + gap / 2 });
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
