//! Rendering and key dispatch for [`crate::appview`] trees.

use crate::appview::*;
use crate::gfx::{Canvas, HAlign, IRect, Rgb};
use crate::skin::Theme;
use crate::strings as tr;
use crate::widgets::{self, Edit, ListState};
use opendeskphone_state::{clock, Key};

/// What a softkey position does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Empty,
    Action(u32, String),
    More,
    Delete,
}

fn softkey_set(tree: &Tree) -> &[Softkey] {
    let mut cur = tree.focus;
    while let Some(id) = cur {
        let Some(n) = tree.node(id) else { break };
        if let Some(keys) = &n.softkeys {
            return keys;
        }
        cur = n.parent;
    }
    &tree.window_softkeys
}

fn focused_input(tree: &Tree) -> Option<WidgetId> {
    tree.focus.filter(|f| tree.node(*f).is_some_and(|n| n.kind == WidgetKind::Input))
}

/// The four softkey positions for the current focus and page.
pub(crate) fn softkey_slots(tree: &Tree) -> [Slot; 4] {
    let set = softkey_set(tree);
    let mut slots = [Slot::Empty, Slot::Empty, Slot::Empty, Slot::Empty];
    if set.len() <= 4 {
        for (i, k) in set.iter().enumerate() {
            slots[i] = Slot::Action(k.id, k.label.clone());
        }
    } else {
        let pages = set.len().div_ceil(3);
        let page = tree.softkey_page as usize % pages;
        for (i, k) in set.iter().skip(page * 3).take(3).enumerate() {
            slots[i] = Slot::Action(k.id, k.label.clone());
        }
        slots[3] = Slot::More;
    }
    // Stock: an Input uses softkey 3 to delete.
    if focused_input(tree).is_some() && slots[2] == Slot::Empty {
        slots[2] = Slot::Delete;
    }
    slots
}

pub(crate) fn softkey_labels(tree: &Tree) -> [String; 4] {
    softkey_slots(tree).map(|s| match s {
        Slot::Empty => String::new(),
        Slot::Action(_, l) => l,
        Slot::More => tr::MORE.to_string(),
        Slot::Delete => tr::DELETE.to_string(),
    })
}

fn halign(a: Align) -> HAlign {
    match a {
        Align::Left => HAlign::Left,
        Align::Center => HAlign::Center,
        Align::Right => HAlign::Right,
    }
}

fn column_widths(cols: &[Column], total: i32) -> Vec<i32> {
    let mut w: Vec<i32> = cols
        .iter()
        .map(|c| match c.width {
            ColumnWidth::Px(p) => p as i32,
            ColumnWidth::Percent(p) => total * p as i32 / 100,
            ColumnWidth::Rest => -1,
        })
        .collect();
    let fixed: i32 = w.iter().filter(|x| **x >= 0).sum();
    let rest = w.iter().filter(|x| **x < 0).count() as i32;
    if rest > 0 {
        let each = ((total - fixed) / rest).max(0);
        for x in w.iter_mut().filter(|x| **x < 0) {
            *x = each;
        }
    }
    w
}

fn is_descendant(tree: &Tree, mut id: WidgetId, ancestor: WidgetId) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        match tree.node(id).and_then(|n| n.parent) {
            Some(p) => id = p,
            None => return false,
        }
    }
}

/// Offset of `id`'s top edge within its ancestor `scroll`'s content.
fn offset_within(tree: &Tree, id: WidgetId, scroll: WidgetId) -> i32 {
    let mut y = 0;
    let mut cur = id;
    while cur != scroll {
        let Some(n) = tree.node(cur) else { break };
        y += n.rect.y as i32;
        match n.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    y
}

fn draw_node(tree: &mut Tree, c: &mut Canvas, th: &Theme, id: WidgetId, ox: i32, oy: i32, now_ms: u64, ink: Rgb) {
    let Some(node) = tree.node(id).cloned() else { return };
    if !node.visible {
        return;
    }
    let r = IRect::new(ox + node.rect.x as i32, oy + node.rect.y as i32, node.rect.w as i32, node.rect.h as i32);
    let old = c.push_clip(r);
    if c.clip().is_empty() {
        c.restore_clip(old);
        return;
    }
    let focused = tree.focus == Some(id);
    if node.kind != WidgetKind::Input
        && let Some(bg) = th.resolve(node.bg, None) {
            c.fill(r, bg);
        }
    let fg = th.resolve(node.fg, Some(ink)).unwrap_or(ink);
    match node.kind {
        WidgetKind::Group => {
            for ch in &node.children {
                draw_node(tree, c, th, *ch, r.x, r.y, now_ms, ink);
            }
        }
        WidgetKind::Scroll => {
            let content_h = node
                .children
                .iter()
                .filter_map(|ch| tree.node(*ch))
                .filter(|n| n.visible)
                .map(|n| n.rect.y as i32 + n.rect.h as i32)
                .max()
                .unwrap_or(0);
            let mut sy = node.scroll_y as i32;
            // Follow the focused descendant.
            if let Some(f) = tree.focus.filter(|f| *f != id && is_descendant(tree, *f, id)) {
                let top = offset_within(tree, f, id);
                let h = tree.node(f).map(|n| n.rect.h as i32).unwrap_or(0);
                if top < sy {
                    sy = top;
                }
                if top + h > sy + r.h {
                    sy = top + h - r.h;
                }
            }
            sy = sy.clamp(0, (content_h - r.h).max(0));
            if let Some(n) = tree.node_mut(id) {
                n.scroll_y = sy as u16;
                n.content_h = content_h.clamp(0, u16::MAX as i32) as u16;
            }
            for ch in &node.children {
                draw_node(tree, c, th, *ch, r.x, r.y - sy, now_ms, ink);
            }
            widgets::scroll_bar(c, th, r, content_h, r.h, sy);
        }
        WidgetKind::Text => {
            let f = th.font(node.font);
            let inner = r;
            if node.wrap {
                let lines = f.wrap(&node.text, inner.w).len() as i32;
                let h = lines * th.line_h;
                let y = match node.valign {
                    VAlign::Top => inner.y,
                    VAlign::Middle => inner.y + ((inner.h - h) / 2).max(0),
                    VAlign::Bottom => inner.y + (inner.h - h).max(0),
                };
                c.text_wrapped(f, IRect::new(inner.x, y, inner.w, inner.bottom() - y), &node.text, fg, halign(node.align), th.line_h);
            } else {
                let lh = f.height().min(inner.h);
                let line = match node.valign {
                    VAlign::Top => IRect { h: lh, ..inner },
                    VAlign::Middle => inner,
                    VAlign::Bottom => IRect { y: inner.bottom() - lh, h: lh, ..inner },
                };
                c.text_in(f, line, &node.text, fg, halign(node.align));
            }
        }
        WidgetKind::Image => match &node.image {
            ImageSource::None => {}
            ImageSource::Icon(name) => {
                if let Some(icon) = th.icon(name) {
                    c.icon_in(icon, r, fg);
                }
            }
            ImageSource::Rgba { width, height, pixels } => {
                let (w, h) = (*width as i32, *height as i32);
                c.rgba(r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, w, h, pixels);
            }
        },
        WidgetKind::Canvas => {
            for op in node.ops.iter() {
                match op {
                    DrawOp::Fill { rect, color } => {
                        if let Some(col) = th.resolve(*color, None) {
                            c.fill(IRect::new(r.x + rect.x as i32, r.y + rect.y as i32, rect.w as i32, rect.h as i32), col);
                        }
                    }
                    DrawOp::Bitmap { x, y, width, height, scale, color, bits } => {
                        if let Some(col) = th.resolve(*color, None) {
                            c.bitmap(r.x + *x as i32, r.y + *y as i32, *width as i32, *height as i32, *scale as i32, bits, col);
                        }
                    }
                    DrawOp::Text { rect, font, color, text } => {
                        if let Some(col) = th.resolve(*color, None) {
                            // The box around the glyphs' pixels goes in
                            // the middle of the rectangle.
                            let font = th.font(*font);
                            let (left, top, w, h) = font.ink(text);
                            let b = IRect::new(r.x + rect.x as i32, r.y + rect.y as i32, rect.w as i32, rect.h as i32);
                            let old = c.push_clip(b);
                            c.text(font, b.x + (b.w - w) / 2 - left, b.y + (b.h - h) / 2 - top, text, col);
                            c.restore_clip(old);
                        }
                    }
                }
            }
        }
        WidgetKind::List => {
            let mut st = ListState { selected: node.selected as usize, top: node.top_row as usize, drag: 0 };
            let bar = node.rows.len() as i32 * node.row_height as i32 > r.h;
            let widths = column_widths(&node.columns, r.w - if bar { th.scroll_w } else { 0 });
            // A list shows its selection whenever it has focus, or when
            // nothing else is focused (single-list screens).
            let show_sel = focused || tree.focus.is_none();
            widgets::list(c, th, r, node.rows.len(), &mut st, node.row_height as i32, show_sel, |c, rr, idx, sel, row_fg| {
                let row_fg = if sel { row_fg } else { fg };
                let mut x = rr.x;
                let cells = &node.rows[idx];
                if node.columns.is_empty() {
                    if let Some(Cell::Text(t)) = cells.first() {
                        c.text_in(th.font(Font::Normal), rr.inset_xy(th.pad, 0), t, row_fg, HAlign::Left);
                    }
                    return;
                }
                for (ci, col) in node.columns.iter().enumerate() {
                    let cr = IRect::new(x, rr.y, widths[ci], rr.h);
                    x += widths[ci];
                    match cells.get(ci) {
                        Some(Cell::Text(t)) => {
                            c.text_in(th.font(col.font), cr.inset_xy(th.pad.min(4), 0), t, row_fg, halign(col.align))
                        }
                        Some(Cell::Icon(name)) => {
                            if let Some(icon) = th.icon(name) {
                                c.icon_in(icon, cr, row_fg);
                            }
                        }
                        Some(Cell::Image(img)) => {
                            let (w, h) = (img.width as i32, img.height as i32);
                            c.rgba(cr.x + (cr.w - w) / 2, cr.y + (cr.h - h) / 2, w, h, &img.pixels);
                        }
                        _ => {}
                    }
                }
            });
            if let Some(n) = tree.node_mut(id) {
                n.selected = st.selected as u32;
                n.top_row = st.top as u32;
            }
        }
        WidgetKind::Input => {
            let pending = focused && tree.multitap.pending(now_ms);
            widgets::text_field(
                c,
                th,
                r,
                &node.text,
                node.cursor as usize,
                node.mode,
                focused,
                pending,
                &node.placeholder,
                node.font,
            );
        }
    }
    if node.border {
        c.frame(r, th.border);
    }
    c.restore_clip(old);
}

/// Paints the tree into `area` (the app content area).
pub(crate) fn render(tree: &mut Tree, c: &mut Canvas, th: &Theme, area: IRect, now_ms: u64) {
    let bg = th.resolve(tree.background, Some(th.bg)).unwrap_or(th.bg);
    c.fill(area, bg);
    render_over(tree, c, th, area, now_ms, th.fg);
}

/// Paints the tree's widgets into `area` over whatever is there (the idle
/// view over the wallpaper).  `ink` is the text color of a widget that
/// sets none.
pub(crate) fn render_over(tree: &mut Tree, c: &mut Canvas, th: &Theme, area: IRect, now_ms: u64, ink: Rgb) {
    let old = c.push_clip(area);
    for id in tree.roots.clone() {
        draw_node(tree, c, th, id, area.x, area.y, now_ms, ink);
    }
    c.restore_clip(old);
}

/// Result of offering a key to an app view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dispatch {
    Consumed,
    Ignored,
    /// `Line(1)`: the UI must background the app.
    Background,
}

fn focusable(tree: &Tree) -> Vec<WidgetId> {
    fn walk(tree: &Tree, id: WidgetId, out: &mut Vec<WidgetId>) {
        let Some(n) = tree.node(id) else { return };
        if !n.visible {
            return;
        }
        if matches!(n.kind, WidgetKind::Input | WidgetKind::List) {
            out.push(id);
        }
        for c in &n.children {
            walk(tree, *c, out);
        }
    }
    let mut out = Vec::new();
    for r in &tree.roots {
        walk(tree, *r, &mut out);
    }
    out
}

pub(crate) fn dispatch(view: &AppView, key: Key, now_ms: u64) -> Dispatch {
    let mut events: Vec<AppEvent> = Vec::new();
    let result = {
        let mut guard = view.batch();
        let tree: &mut Tree = &mut guard;
        dispatch_locked(tree, key, now_ms, &mut events)
    };
    for ev in events {
        view.push_event(ev);
    }
    result
}

fn dispatch_locked(tree: &mut Tree, key: Key, now_ms: u64, events: &mut Vec<AppEvent>) -> Dispatch {
    if tree.capture {
        // Every key is the app's, but the user can always leave: Cancel
        // three times in a row.
        tree.cancels = if key == Key::Cancel { tree.cancels + 1 } else { 0 };
        if tree.cancels >= 3 {
            tree.cancels = 0;
            return Dispatch::Background;
        }
        events.push(AppEvent::Key { widget: None, key });
        return Dispatch::Consumed;
    }
    match key {
        Key::Line(1) => return Dispatch::Background,
        Key::VolumeUp | Key::VolumeDown | Key::Mute | Key::Speaker | Key::Headset => return Dispatch::Ignored,
        // A full-screen view has no softkey bar: its softkey keys are
        // keys like any other.
        Key::Soft(n @ 1..=4) if !tree.full_screen => {
            return match softkey_slots(tree)[n as usize - 1].clone() {
                Slot::Empty => Dispatch::Ignored,
                Slot::Action(id, _) => {
                    events.push(AppEvent::Softkey { id });
                    Dispatch::Consumed
                }
                Slot::More => {
                    tree.softkey_page = tree.softkey_page.wrapping_add(1);
                    let pages = softkey_set(tree).len().div_ceil(3).max(1);
                    tree.softkey_page %= pages as u8;
                    Dispatch::Consumed
                }
                Slot::Delete => {
                    if let Some(id) = focused_input(tree) {
                        let mut tap = std::mem::take(&mut tree.multitap);
                        if let Some(n) = tree.node_mut(id)
                            && widgets::edit_backspace(&mut n.text, &mut n.cursor, &mut tap) == Edit::Changed {
                                events.push(AppEvent::InputChanged { widget: id, value: n.text.clone() });
                            }
                        tree.multitap = tap;
                    }
                    Dispatch::Consumed
                }
            };
        }
        _ => {}
    }

    // Claims: focused widget, its parents, then the window.
    let mut cur = tree.focus;
    while let Some(id) = cur {
        let Some(n) = tree.node(id) else { break };
        if n.claims.claims(key) {
            events.push(AppEvent::Key { widget: Some(id), key });
            return Dispatch::Consumed;
        }
        cur = n.parent;
    }
    if tree.window_claims.claims(key) {
        events.push(AppEvent::Key { widget: None, key });
        return Dispatch::Consumed;
    }

    // Built-in behavior of the focused widget (or of a lone list).
    let target = tree.focus.or_else(|| {
        let f = focusable(tree);
        (f.len() == 1).then(|| f[0])
    });
    if let Some(id) = target {
        let kind = tree.node(id).map(|n| n.kind);
        match kind {
            Some(WidgetKind::List) => {
                let n = tree.node_mut(id).unwrap();
                let len = n.rows.len();
                match key {
                    Key::Up | Key::Down => {
                        let mut st = ListState { selected: n.selected as usize, top: n.top_row as usize, drag: 0 };
                        if st.step(if key == Key::Up { -1 } else { 1 }, len, n.wrap_selection) {
                            n.selected = st.selected as u32;
                            events.push(AppEvent::ListSelected { widget: id, row: n.selected });
                        }
                        return Dispatch::Consumed;
                    }
                    Key::Select if len > 0 => {
                        events.push(AppEvent::ListActivated { widget: id, row: n.selected });
                        return Dispatch::Consumed;
                    }
                    Key::Digit(d) if n.shortcuts => {
                        let row = if d == 0 { 9 } else { d as usize - 1 };
                        if row < len {
                            n.selected = row as u32;
                            events.push(AppEvent::ListActivated { widget: id, row: row as u32 });
                        }
                        return Dispatch::Consumed;
                    }
                    _ => {}
                }
            }
            Some(WidgetKind::Input) => match key {
                Key::Select => {
                    tree.multitap.reset();
                    let value = tree.node(id).map(|n| n.text.clone()).unwrap_or_default();
                    events.push(AppEvent::InputSubmitted { widget: id, value });
                    return Dispatch::Consumed;
                }
                Key::Up | Key::Down => {
                    let order = focusable(tree);
                    if let Some(pos) = order.iter().position(|w| *w == id) {
                        let next = if key == Key::Up { pos.checked_sub(1) } else { Some(pos + 1) };
                        if let Some(w) = next.and_then(|i| order.get(i)).copied() {
                            tree.focus(Some(w));
                            events.push(AppEvent::Focused { widget: w });
                        }
                    }
                    return Dispatch::Consumed;
                }
                _ => {
                    let mut tap = std::mem::take(&mut tree.multitap);
                    let n = tree.node_mut(id).unwrap();
                    let e = widgets::edit_key(&mut n.text, &mut n.cursor, n.mode, n.max_len as usize, &mut tap, key, now_ms);
                    let value = n.text.clone();
                    tree.multitap = tap;
                    match e {
                        Edit::Changed => {
                            events.push(AppEvent::InputChanged { widget: id, value });
                            return Dispatch::Consumed;
                        }
                        Edit::Consumed => return Dispatch::Consumed,
                        Edit::Ignored => {}
                    }
                }
            },
            Some(WidgetKind::Scroll) => {
                let line = tree.metrics.line_height as i32;
                let n = tree.node_mut(id).unwrap();
                let max = (n.content_h as i32 - n.rect.h as i32).max(0);
                match key {
                    Key::Up => {
                        n.scroll_y = (n.scroll_y as i32 - line).max(0) as u16;
                        return Dispatch::Consumed;
                    }
                    Key::Down => {
                        n.scroll_y = (n.scroll_y as i32 + line).min(max) as u16;
                        return Dispatch::Consumed;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    // A list that lost focus to an input above it: Up/Down from a list at
    // its ends could move focus, but stock lists wrap instead, so nothing
    // more here.
    if key == Key::Cancel {
        events.push(AppEvent::CloseRequested);
        return Dispatch::Consumed;
    }
    Dispatch::Ignored
}

pub(crate) fn dispatch_key(view: &AppView, key: Key) -> bool {
    dispatch(view, key, clock::mono_ms()) == Dispatch::Consumed
}
