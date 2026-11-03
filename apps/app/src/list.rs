//! The chat's virtualized list: only the rows near the screen are in the page, with spacers for
//! the rest. Row heights are guessed when a row arrives and replaced by the real ones once it has
//! been drawn; chat-core's splices keep them in step with the rows, so a page of older messages
//! landing at the top keeps every height below it. [`Scroll`] is ChatView.tsx's scroll rules:
//! follow new output while at the bottom, keep the row being read in place otherwise.

use inyon_chat_core::chat::{Part, Row};
use inyon_chat_core::{ListRow, Splice};

/// Rows drawn beyond each edge of the screen, in pixels, so a quick flick doesn't show blanks.
pub const OVERSCAN: f64 = 900.0;
/// The screen height assumed before the first measurement.
const VIEW_GUESS: f64 = 900.0;

#[derive(Debug, Clone, PartialEq)]
struct Slot {
    key: String,
    h: f64,
}

/// The rows' keys and heights, in the order the list shows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VList {
    slots: Vec<Slot>,
}

/// About how tall a row draws (its gap below included), until it has been drawn.
pub fn guess(row: &ListRow) -> f64 {
    // lines of text at the chat's width, about 70 characters each
    let lines = |t: &str| t.lines().map(|l| (l.chars().count() / 70 + 1) as f64).sum::<f64>().max(1.0);
    match row {
        ListRow::Row(Row::User { item: inyon_protocol::ChatItem::User { text, .. }, .. }) => 70.0 + 24.0 * lines(text).min(14.0),
        ListRow::Row(Row::Assistant(t)) => {
            let parts: f64 = t
                .parts
                .iter()
                .map(|p| match p {
                    Part::Text { text, .. } => 26.0 * lines(text) + 20.0,
                    Part::Tools { calls, .. } => 34.0 * calls.len().min(6) as f64 + 12.0,
                    Part::Special { .. } => 160.0,
                    Part::Thinking { .. } | Part::Notice { .. } => 36.0,
                })
                .sum();
            parts + 48.0
        }
        ListRow::Pending(p) => 70.0 + 24.0 * lines(&p.text).min(14.0),
        ListRow::Queued { text, .. } | ListRow::Held { prompt: inyon_protocol::HeldPrompt { text, .. }, .. } => 70.0 + 24.0 * lines(text).min(14.0),
        ListRow::Live { items } => items.iter().map(|i| 26.0 * lines(&i.text)).sum::<f64>() + 20.0,
        _ => 52.0,
    }
}

/// The rows to draw and the room the others take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub end: usize,
    /// pixels above the first drawn row and below the last
    pub before: u32,
    pub after: u32,
}

impl VList {
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Follows the rows' splices (see [`inyon_chat_core::apply_splices`]).
    pub fn splice(&mut self, splices: &[Splice]) {
        for s in splices {
            match s {
                Splice::Insert { at, rows } => {
                    let at = (*at).min(self.slots.len());
                    self.slots.splice(at..at, rows.iter().map(|r| Slot { key: r.key().into_owned(), h: guess(r) }));
                }
                // the same row changed: its last height stays the best guess until it's drawn again
                Splice::Replace { at, row } => {
                    if let Some(s) = self.slots.get_mut(*at) {
                        let key = row.key();
                        if s.key != key {
                            *s = Slot { key: key.into_owned(), h: guess(row) };
                        }
                    }
                }
                Splice::Remove { at, count } => {
                    let at = (*at).min(self.slots.len());
                    let end = (at + count).min(self.slots.len());
                    self.slots.drain(at..end);
                }
            }
        }
    }

    /// A drawn row's height; true when it changed.
    pub fn measured(&mut self, key: &str, h: f64) -> bool {
        match self.slots.iter_mut().rev().find(|s| s.key == key) {
            Some(s) if (s.h - h).abs() >= 0.5 => {
                s.h = h;
                true
            }
            _ => false,
        }
    }

    /// Whether `h` would change the height noted for `key`.
    pub fn differs(&self, key: &str, h: f64) -> bool {
        self.slots.iter().rev().find(|s| s.key == key).is_some_and(|s| (s.h - h).abs() >= 0.5)
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.slots.iter().rposition(|s| s.key == key)
    }

    pub fn key(&self, i: usize) -> Option<&str> {
        self.slots.get(i).map(|s| s.key.as_str())
    }

    /// Where row `i` starts.
    pub fn offset(&self, i: usize) -> f64 {
        self.slots[..i.min(self.slots.len())].iter().map(|s| s.h).sum()
    }

    pub fn total(&self) -> f64 {
        self.offset(self.slots.len())
    }

    /// The row at height `y` (the last one past the end).
    pub fn index_at(&self, y: f64) -> usize {
        let mut at = 0.0;
        for (i, s) in self.slots.iter().enumerate() {
            at += s.h;
            if at > y {
                return i;
            }
        }
        self.slots.len().saturating_sub(1)
    }

    /// The rows from `top` to `top + view` and `overscan` either side.
    pub fn window(&self, top: f64, view: f64, overscan: f64) -> Window {
        let (lo, hi) = (top - overscan, top + view + overscan);
        let (mut start, mut end) = (self.slots.len(), self.slots.len());
        let (mut before, mut at) = (0.0, 0.0);
        for (i, s) in self.slots.iter().enumerate() {
            let next = at + s.h;
            if start == self.slots.len() && next > lo {
                start = i;
                before = at;
            }
            if at >= hi {
                end = i;
                break;
            }
            at = next;
        }
        if start == self.slots.len() {
            // scrolled past everything (or nothing to show): the last rows
            start = self.slots.len().saturating_sub(1).min(end);
            before = self.offset(start);
        }
        let shown: f64 = self.slots[start..end].iter().map(|s| s.h).sum();
        let after = (self.total() - before - shown).max(0.0);
        Window { start, end, before: before.round() as u32, after: after.round() as u32 }
    }
}

/// Where the reader is in the list, and whether to follow new output.
#[derive(Debug, Clone, PartialEq)]
pub struct Scroll {
    /// the scroll position, in the list's own coordinates (0 is the list's first row)
    pub top: f64,
    pub view: f64,
    /// at the bottom: new output scrolls into view
    pub stick: bool,
    /// the first row on screen, and how far down into it the screen starts
    anchor: Option<(String, f64)>,
    last_top: f64,
    /// when the reader last touched, wheeled or keyed (ms): only then is a move up theirs
    input_at: f64,
}

impl Default for Scroll {
    fn default() -> Self {
        Scroll { top: 0.0, view: VIEW_GUESS, stick: true, anchor: None, last_top: 0.0, input_at: f64::NEG_INFINITY }
    }
}

/// What a scroll asks of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scrolled {
    /// near the top and going up: fetch earlier messages before the reader gets there
    pub more: bool,
    /// far enough up that the jump-to-latest button shows
    pub away: bool,
}

impl Scroll {
    /// The reader wheeled, touched or pressed a key; `up` when it moved the conversation back.
    pub fn input(&mut self, now: f64, up: bool) {
        self.input_at = now;
        if up {
            self.stick = false;
        }
    }

    /// The list scrolled to `top` (list coordinates) with `view` pixels showing; `room` is how far
    /// the bottom of the content is below the bottom of the screen.
    pub fn scrolled(&mut self, list: &VList, top: f64, view: f64, room: f64, now: f64) -> Scrolled {
        self.view = view;
        if room < 24.0 {
            self.stick = true;
        } else if top < self.last_top - 2.0 && now - self.input_at < 1200.0 {
            self.stick = false;
        }
        let more = !self.stick && top < self.last_top && top < view.max(400.0);
        self.last_top = top;
        self.top = top;
        self.keep(list);
        Scrolled { more, away: room >= 80.0 }
    }

    /// Notes the row on screen, to keep it in place when what's above it changes.
    pub fn keep(&mut self, list: &VList) {
        if list.len() == 0 {
            self.anchor = None;
            return;
        }
        let i = list.index_at(self.top.max(0.0));
        self.anchor = list.key(i).map(|k| (k.to_string(), self.top - list.offset(i)));
    }

    /// Where the list should be scrolled now that rows or heights changed: the bottom while
    /// following, else wherever keeps the noted row where it was.
    pub fn settle(&mut self, list: &VList) -> f64 {
        let bottom = (list.total() - self.view).max(0.0);
        self.top = if self.stick {
            bottom
        } else {
            match self.anchor.as_ref().and_then(|(k, d)| list.index_of(k).map(|i| list.offset(i) + d)) {
                Some(t) => t.clamp(0.0, bottom),
                None => self.top.min(bottom),
            }
        };
        self.last_top = self.top;
        if !self.stick && self.anchor.as_ref().is_none_or(|(k, _)| list.index_of(k).is_none()) {
            self.keep(list);
        }
        self.top
    }

    /// The rows to draw.
    pub fn window(&self, list: &VList) -> Window {
        let top = if self.stick { (list.total() - self.view).max(0.0) } else { self.top };
        list.window(top, self.view, OVERSCAN)
    }

    /// Follow from the bottom again (a message sent from here, the jump button, a reopened chat).
    pub fn follow(&mut self) {
        self.stick = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_chat_core::{apply_splices, Chat};
    use inyon_protocol::ServerMsg;
    use serde_json::json;

    fn user(seq: i64, text: &str) -> serde_json::Value {
        json!({ "kind": "user", "seq": seq, "uuid": format!("u{seq}"), "ts": "2026-10-31T12:00:00.000Z", "text": text, "images": 0 })
    }
    fn reply(seq: i64, text: &str) -> serde_json::Value {
        json!({ "kind": "assistant", "seq": seq, "uuid": format!("a{seq}"), "ts": "2026-10-31T12:00:01.000Z", "msgId": format!("m{seq}"), "block": { "type": "text", "text": text } })
    }
    fn info() -> serde_json::Value {
        json!({ "hostId": "box", "paneId": "%1", "running": true, "queue": [], "tasks": [], "subagents": [], "artifacts": [], "stats": { "turns": 1, "compactions": 0 } })
    }
    fn msg(v: serde_json::Value) -> ServerMsg {
        serde_json::from_value(v).unwrap()
    }
    /// a chat of `n` exchanges from seq `from`
    fn page(from: i64, n: i64) -> Vec<serde_json::Value> {
        (0..n).flat_map(|i| [user(from + 2 * i, &format!("question {i}")), reply(from + 2 * i + 1, &format!("answer {i}"))]).collect()
    }

    #[test]
    fn splices_keep_the_heights_with_their_rows() {
        let mut chat = Chat::new();
        let mut list = VList::default();
        let mut rows = vec![];
        let s = chat.apply(&msg(json!({ "t": "chat.reset", "key": "box:%1", "info": info(), "items": page(100, 3), "hasMore": true })), 0);
        apply_splices(&mut rows, &s);
        list.splice(&s);
        assert_eq!(list.len(), rows.len());
        for (i, r) in rows.iter().enumerate() {
            assert!(list.differs(&r.key(), 100.0 + i as f64));
            assert!(list.measured(&r.key(), 100.0 + i as f64));
        }
        assert!(!list.differs(&rows[0].key(), 100.2));
        assert!(!list.measured(&rows[0].key(), 100.2), "under half a pixel is no change");
        // older messages come in at the top: every height below stays with its row
        let s = chat.apply(&msg(json!({ "t": "chat.more", "key": "box:%1", "items": page(90, 2), "hasMore": false })), 0);
        apply_splices(&mut rows, &s);
        list.splice(&s);
        assert_eq!(list.len(), rows.len());
        let last = rows.last().unwrap().key().into_owned();
        let i = list.index_of(&last).unwrap();
        assert_eq!(list.offset(i + 1) - list.offset(i), 105.0);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(list.key(i), Some(r.key().as_ref()));
        }
        // a new message at the end, and the working line going
        let s = chat.apply(&msg(json!({ "t": "chat.append", "key": "box:%1", "items": [user(106, "and now?")] })), 0);
        apply_splices(&mut rows, &s);
        list.splice(&s);
        assert_eq!(list.key(list.len() - 1), Some(rows.last().unwrap().key().as_ref()));
    }

    fn even(n: usize, h: f64) -> VList {
        VList { slots: (0..n).map(|i| Slot { key: format!("r{i}"), h }).collect() }
    }

    #[test]
    fn only_the_rows_near_the_screen_are_drawn() {
        let list = even(1000, 100.0);
        let w = list.window(50_000.0, 800.0, 900.0);
        assert_eq!((w.start, w.end), (491, 517));
        assert_eq!(w.before, 49_100);
        assert_eq!(w.before as f64 + (w.end - w.start) as f64 * 100.0 + w.after as f64, 100_000.0);
        let top = list.window(0.0, 800.0, 900.0);
        assert_eq!((top.start, top.before), (0, 0));
        let short = even(3, 100.0).window(0.0, 800.0, 900.0);
        assert_eq!((short.start, short.end, short.after), (0, 3, 0));
        let empty = VList::default().window(0.0, 800.0, 900.0);
        assert_eq!((empty.start, empty.end), (0, 0));
        // following: the last screenful, whatever the old position
        let s = Scroll { view: 800.0, ..Scroll::default() };
        let w = s.window(&list);
        assert_eq!(w.end, 1000);
        assert!(w.start < 992 && w.start > 970, "{w:?}");
    }

    #[test]
    fn reading_back_keeps_the_row_in_place_as_older_ones_arrive() {
        let mut list = even(50, 100.0);
        let mut s = Scroll::default();
        s.input(1000.0, true);
        let away = s.scrolled(&list, 1250.0, 800.0, 2950.0, 1000.0);
        assert!(!s.stick && away.away);
        // twenty earlier rows come in above, and the row read stays where it was on screen
        list.slots.splice(0..0, (0..20).map(|i| Slot { key: format!("old{i}"), h: 80.0 }));
        assert_eq!(s.settle(&list), 1600.0 + 1250.0);
        // a row above the one read turns out taller once drawn
        assert!(list.measured("r5", 160.0));
        assert_eq!(s.settle(&list), 2850.0 + 60.0);
    }

    #[test]
    fn following_stops_on_a_move_up_and_starts_again_at_the_bottom() {
        let list = even(50, 100.0);
        let mut s = Scroll { view: 800.0, ..Scroll::default() };
        assert_eq!(s.settle(&list), 4200.0);
        // content shrinking below moves the position up, but nobody touched anything
        s.scrolled(&list, 4100.0, 800.0, 100.0, 10_000.0);
        assert!(s.stick);
        // the reader scrolls up
        s.input(20_000.0, false);
        let r = s.scrolled(&list, 3000.0, 800.0, 1200.0, 20_100.0);
        assert!(!s.stick && r.away && !r.more);
        // near the top and still going up: earlier messages are asked for
        s.input(21_000.0, false);
        assert!(s.scrolled(&list, 300.0, 800.0, 3900.0, 21_000.0).more);
        // back down to the bottom
        s.scrolled(&list, 4190.0, 800.0, 10.0, 22_000.0);
        assert!(s.stick);
        // a wheel up stops following before any scroll event
        s.input(23_000.0, true);
        assert!(!s.stick);
        s.follow();
        assert!(s.stick);
    }

    #[test]
    fn guesses_grow_with_the_text() {
        let short = ListRow::Queued { index: 0, text: "hi".into() };
        let long = ListRow::Queued { index: 0, text: "x\n".repeat(10) };
        assert!(guess(&long) > guess(&short));
    }
}
