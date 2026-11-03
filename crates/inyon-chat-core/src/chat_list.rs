//! One conversation as the chat screen shows it: the state `store.ts` keeps per chat, the reducer
//! that applies the hub's chat messages to it, and the rows `ChatView.tsx` draws from it, kept as a
//! list that changes by [`Splice`]s so a virtualized list can update in place.
//!
//! [`ChatData::reduce`] is store.ts's `onMessage` for one chat, message for message. [`Chat`] wraps
//! it with the list: after each message it builds the rows the web app's ChatView renders
//! (transcript rows, then this device's unsent bubbles, the agent's queue, the held messages, the
//! catching-up bar, the live reply, the working line and the background line) and returns how the
//! list changed.

use std::borrow::Cow;
use std::ops::Deref;

use inyon_protocol::{ChatInfo, ChatItem, DraftSource, HeldPrompt, LiveItem, LiveKind, ScreenState, SendState, ServerMsg};
use serde::{Deserialize, Serialize};

use crate::chat::{build_rows, Row};
use crate::js::trim;
use crate::pending::{msg_key, others_sends, settle, PendingMessage, PendingStatus, PENDING_MS};

/// The unsent message, shared with other devices and the terminal's input box.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub text: String,
    pub source: DraftSource,
    pub at: u64,
}

/// A chat as store.ts keeps it (`ChatState`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatData {
    pub info: Option<ChatInfo>,
    pub items: Vec<ChatItem>,
    pub has_more: bool,
    pub loaded: bool,
    pub screen: Option<ScreenState>,
    /// what the agent is writing right now, shown until its record has it
    pub live: Option<Vec<LiveItem>>,
    /// when earlier messages last came back (with or without any), so a wait for them can end
    pub more_at: Option<u64>,
    /// catching up on what was said while away, as a fraction read; cleared by the next reset
    pub catching_up: Option<f64>,
    /// sent from this device, not yet seen in the transcript
    pub pending: Vec<PendingMessage>,
    /// the unsent message, shared with other devices and the terminal's input box
    pub draft: Option<Draft>,
    /// drawn from the copy kept on this device; the hub's first page replaces it
    pub stale: bool,
}

/// The page of a chat kept on the device (snapshot.ts), so the app paints real content in its
/// first frame. Field names as the web app stores them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<ChatInfo>,
    pub items: Vec<ChatItem>,
    pub has_more: bool,
}

/// A chat page holds at most this many messages.
const SNAPSHOT_ITEMS: usize = 40;

/// The chat a hub message is about, for chat messages.
pub fn chat_key_of(msg: &ServerMsg) -> Option<&str> {
    match msg {
        ServerMsg::ChatReset { key, .. }
        | ServerMsg::ChatAppend { key, .. }
        | ServerMsg::ChatMore { key, .. }
        | ServerMsg::ChatInfo { key, .. }
        | ServerMsg::ChatScreen { key, .. }
        | ServerMsg::ChatLive { key, .. }
        | ServerMsg::ChatProgress { key, .. }
        | ServerMsg::ChatDraft { key, .. } => Some(key),
        _ => None,
    }
}

/// Live text whose finished record is among `items` goes: the record (same id) takes its place in
/// the same render, so the reply never shows twice and never blinks out between the two.
pub fn without_recorded(live: Option<Vec<LiveItem>>, items: &[ChatItem]) -> Option<Vec<LiveItem>> {
    let live = live?;
    if live.is_empty() || items.is_empty() {
        return Some(live);
    }
    let mut ids = std::collections::HashSet::new();
    for i in items {
        ids.insert(i.uuid());
        match i {
            ChatItem::Assistant { msg_id, block, .. } => {
                ids.insert(msg_id);
                if let inyon_protocol::AssistantBlock::ToolUse { id, .. } = block {
                    ids.insert(id);
                }
            }
            ChatItem::ToolResult { tool_use_id, .. } => {
                ids.insert(tool_use_id);
            }
            _ => {}
        }
    }
    Some(live.into_iter().filter(|l| !ids.contains(l.id.as_str())).collect())
}

/// Text the hub read off the screen (id "screen:…") has no record with its id. A new reply on record
/// takes its place now; the hub sends whatever of it is still unrecorded a moment later, so the
/// reply never shows twice in between.
pub fn without_screen_read(live: Option<Vec<LiveItem>>, fresh: &[ChatItem]) -> Option<Vec<LiveItem>> {
    let live = live?;
    if !live.iter().any(|l| l.id.starts_with("screen:")) {
        return Some(live);
    }
    if !fresh.iter().any(|i| matches!(i, ChatItem::Assistant { block: inyon_protocol::AssistantBlock::Text { .. }, .. })) {
        return Some(live);
    }
    Some(live.into_iter().filter(|l| !l.id.starts_with("screen:")).collect())
}

pub(crate) fn sends(info: Option<&ChatInfo>) -> &[SendState] {
    info.and_then(|i| i.sends.as_deref()).unwrap_or(&[])
}

impl ChatData {
    /// store.ts's handling of one chat's hub message; `now` is unix ms. Returns whether the
    /// transcript itself changed, and nothing happens for a message that isn't about a chat.
    pub fn reduce(&mut self, msg: &ServerMsg, now: u64) -> bool {
        match msg {
            ServerMsg::ChatReset { info, items, has_more, .. } => {
                // Gemini, Antigravity and OpenCode resend their latest page on every step: keep the
                // earlier messages this screen already loaded, or someone reading back jumps on each update
                let keep = items.first().is_some_and(|head| *has_more && self.items.iter().any(|i| i.seq() == head.seq() && i.uuid() == head.uuid()));
                // what this reset brings that wasn't here before, for text read off the screen
                let fresh: Vec<ChatItem> = if self.loaded {
                    let known: std::collections::HashSet<&str> = self.items.iter().map(ChatItem::uuid).collect();
                    items.iter().filter(|i| !known.contains(i.uuid())).cloned().collect()
                } else {
                    Vec::new()
                };
                let next = if keep {
                    let head = items[0].seq();
                    let mut kept: Vec<ChatItem> = self.items.drain(..).filter(|i| i.seq() < head).collect();
                    kept.extend(items.iter().cloned());
                    kept
                } else {
                    items.clone()
                };
                self.pending = settle(&self.pending, items, sends(Some(info)));
                self.live = without_screen_read(without_recorded(self.live.take(), items), &fresh);
                self.info = Some((**info).clone());
                self.items = next;
                if !keep {
                    self.has_more = *has_more;
                }
                self.loaded = true;
                self.stale = false;
                self.catching_up = None;
                true
            }
            ServerMsg::ChatLive { items, .. } => {
                self.live = without_recorded(Some(items.clone()), &self.items);
                false
            }
            ServerMsg::ChatProgress { done, total, .. } => {
                self.catching_up = (*total > 0).then(|| (*done as f64 / *total as f64).min(1.0));
                false
            }
            ServerMsg::ChatAppend { items, .. } => {
                let last = self.items.last().map_or(-1, ChatItem::seq);
                let fresh: Vec<ChatItem> = items.iter().filter(|i| i.seq() > last).cloned().collect();
                self.pending = settle(&self.pending, &fresh, sends(self.info.as_ref()));
                self.live = without_screen_read(without_recorded(self.live.take(), &fresh), &fresh);
                let changed = !fresh.is_empty();
                self.items.extend(fresh);
                changed
            }
            ServerMsg::ChatMore { items, has_more, .. } => {
                let first = self.items.first().map_or(i64::MAX, ChatItem::seq);
                let older: Vec<ChatItem> = items.iter().filter(|i| i.seq() < first).cloned().collect();
                let changed = !older.is_empty();
                self.items.splice(0..0, older);
                self.has_more = *has_more;
                self.more_at = Some(now);
                changed
            }
            ServerMsg::ChatInfo { info, .. } => {
                self.loaded = self.loaded || info.transcript_path.as_deref().is_none_or(str::is_empty);
                self.info = Some((**info).clone());
                false
            }
            ServerMsg::ChatScreen { screen, .. } => {
                self.screen = Some((**screen).clone());
                false
            }
            ServerMsg::ChatDraft { text, source, at, .. } => {
                self.draft = Some(Draft { text: text.clone(), source: *source, at: *at });
                false
            }
            _ => false,
        }
    }

    /// A chat nobody watched for a minute lets its messages go; they reload on return. Its title
    /// and model can show while it reloads, but a goal or queue may be long gone by then.
    pub fn forget(&mut self) {
        self.items.clear();
        self.has_more = false;
        self.loaded = false;
        self.screen = None;
        if let Some(info) = &mut self.info {
            info.goal = None;
            info.queue.clear();
        }
    }

    /// The page to keep on the device, once the hub has sent a real one: its last 40 messages.
    pub fn snapshot(&self) -> Option<ChatSnapshot> {
        if !self.loaded || self.stale || self.items.is_empty() {
            return None;
        }
        let items = self.items[self.items.len().saturating_sub(SNAPSHOT_ITEMS)..].to_vec();
        Some(ChatSnapshot { info: self.info.clone(), has_more: self.has_more || items.len() < self.items.len(), items })
    }

    /// Shows the page kept on the device until the hub sends the real one.
    pub fn hydrate(&mut self, kept: ChatSnapshot) {
        if self.loaded {
            return;
        }
        self.info = kept.info;
        self.items = kept.items;
        self.has_more = kept.has_more;
        self.loaded = true;
        self.stale = true;
    }
}

/// One row of the chat list.
// nearly every row of a long chat is a transcript row, so boxing that variant would add an
// allocation per row and save nothing
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ListRow {
    /// a row of the transcript (a user bubble, an assistant turn, a command, a notice, a goal)
    Row(Row),
    /// a message from this device, or another one, not in the conversation yet
    Pending(PendingMessage),
    /// a message waiting in the agent's own queue
    Queued { index: usize, text: String },
    /// a message Inyon holds until the agent is free; `index` of `count`, in the order they go
    Held { prompt: HeldPrompt, index: usize, count: usize },
    /// reading what was said while nobody watched, as a fraction read
    CatchingUp { done: f64 },
    /// what the agent is writing right now
    Live { items: Vec<LiveItem> },
    /// the agent is working: its spinner text, when the turn started (unix ms) and the running
    /// command's latest output. `joins_turn` when it sits right under an open turn or live text.
    Working { activity: Option<String>, since: Option<f64>, output: Option<String>, joins_turn: bool },
    /// idle, with work still running in the background (the footer's text, e.g. "1 shell")
    Background { text: String },
}

impl ListRow {
    /// A key that stays with the row while it changes, unique in the list.
    pub fn key(&self) -> Cow<'_, str> {
        match self {
            ListRow::Row(r) => Cow::Borrowed(r.key()),
            ListRow::Pending(p) => Cow::Owned(format!("p{}", p.id)),
            ListRow::Queued { index, text } => Cow::Owned(format!("q{index}:{text}")),
            ListRow::Held { prompt, .. } => Cow::Owned(format!("h{}", prompt.id)),
            ListRow::CatchingUp { .. } => Cow::Borrowed("reading"),
            ListRow::Live { .. } => Cow::Borrowed("live"),
            ListRow::Working { .. } => Cow::Borrowed("working"),
            ListRow::Background { .. } => Cow::Borrowed("background"),
        }
    }
}

/// One change to the list, applied in order: indexes are in the list as the splices before it
/// left it.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Splice {
    Insert { at: usize, rows: Vec<ListRow> },
    Replace { at: usize, row: ListRow },
    Remove { at: usize, count: usize },
}

/// Applies splices to a copy of the list, for a view that keeps its own.
pub fn apply_splices(list: &mut Vec<ListRow>, splices: &[Splice]) {
    for s in splices {
        match s {
            Splice::Insert { at, rows } => {
                list.splice(*at..*at, rows.iter().cloned());
            }
            Splice::Replace { at, row } => list[*at] = row.clone(),
            Splice::Remove { at, count } => {
                list.drain(*at..*at + *count);
            }
        }
    }
}

/// The splices that turn `old` into `new`: rows matched by key from both ends are replaced where
/// they changed, and what's between is removed and inserted.
pub fn diff(old: &[ListRow], new: &[ListRow]) -> Vec<Splice> {
    let (ol, nl) = (old.len(), new.len());
    let mut out = Vec::new();
    let mut i = 0;
    while i < ol && i < nl && old[i].key() == new[i].key() {
        if old[i] != new[i] {
            out.push(Splice::Replace { at: i, row: new[i].clone() });
        }
        i += 1;
    }
    let mut j = 0;
    while j < ol - i && j < nl - i && old[ol - 1 - j].key() == new[nl - 1 - j].key() {
        j += 1;
    }
    if ol - j > i {
        out.push(Splice::Remove { at: i, count: ol - j - i });
    }
    if nl - j > i {
        out.push(Splice::Insert { at: i, rows: new[i..nl - j].to_vec() });
    }
    for k in 0..j {
        let (o, n) = (ol - j + k, nl - j + k);
        if old[o] != new[n] {
            out.push(Splice::Replace { at: n, row: new[n].clone() });
        }
    }
    out
}

/// A conversation with its rows. It reads as its [`ChatData`]; every change goes through a method
/// that returns how the rows changed.
#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub(crate) data: ChatData,
    /// the transcript's rows, rebuilt only when the transcript changes
    transcript: Vec<Row>,
    rows: Vec<ListRow>,
    /// how many of `rows` are transcript rows
    shown: usize,
    /// where it is in its `/ws2` stream, and that stream's send queue
    pub(crate) stream: crate::v2::ChatStream,
}

impl Deref for Chat {
    type Target = ChatData;
    fn deref(&self) -> &ChatData {
        &self.data
    }
}

impl Chat {
    pub fn new() -> Self {
        Self::default()
    }

    /// A chat that starts with the sends this device still had pending (see `recover_pending`).
    pub fn with_pending(pending: Vec<PendingMessage>) -> Self {
        Chat { data: ChatData { pending, ..ChatData::default() }, ..Self::default() }
    }

    /// The rows as they stand.
    pub fn rows(&self) -> &[ListRow] {
        &self.rows
    }

    /// Applies a hub message about this chat.
    pub fn apply(&mut self, msg: &ServerMsg, now: u64) -> Vec<Splice> {
        let changed = self.data.reduce(msg, now);
        self.settled(changed, now)
    }

    /// Changes the chat directly (a message sent, retried or dismissed; the page kept on the device),
    /// as store.ts's `patchChat` does.
    pub fn patch(&mut self, now: u64, f: impl FnOnce(&mut ChatData)) -> Vec<Splice> {
        f(&mut self.data);
        self.settled(true, now)
    }

    /// Time passing: a send the agent still hasn't written after ten minutes says so, and another
    /// device's send older than five minutes stops showing. The web app checks every five seconds.
    pub fn tick(&mut self, now: u64) -> Vec<Splice> {
        for p in &mut self.data.pending {
            if p.status == PendingStatus::Sent && !p.queued && now.saturating_sub(p.at) >= PENDING_MS {
                p.status = PendingStatus::Failed;
                p.error = Some("Still not in the conversation after 10 minutes. Check the terminal before retrying.".into());
            }
        }
        self.settled(false, now)
    }

    /// Lets the messages go after a minute unwatched (see [`ChatData::forget`]).
    pub fn forget(&mut self, now: u64) -> Vec<Splice> {
        self.patch(now, ChatData::forget)
    }

    pub(crate) fn settled(&mut self, transcript_changed: bool, now: u64) -> Vec<Splice> {
        // the hub's ledger (info.sends) can settle a send at any time, not only when the
        // transcript grows; ChatView does this on every change
        if !self.data.pending.is_empty() {
            self.data.pending = settle(&self.data.pending, &[], sends(self.data.info.as_ref()));
        }
        if transcript_changed {
            self.transcript = build_rows(&self.data.items);
        }
        let Some(tail) = self.tail(now) else {
            let splices = diff(&self.rows, &[]);
            self.rows.clear();
            self.shown = 0;
            return splices;
        };
        let shown = self.transcript.len();
        if !transcript_changed && self.shown == shown {
            // the transcript's rows are as they were: only what follows them can have changed (the
            // live reply, the working line), and a long chat isn't copied or compared on each update
            let mut splices = diff(&self.rows[shown..], &tail);
            for s in &mut splices {
                match s {
                    Splice::Insert { at, .. } | Splice::Replace { at, .. } | Splice::Remove { at, .. } => *at += shown,
                }
            }
            self.rows.truncate(shown);
            self.rows.extend(tail);
            return splices;
        }
        let rows: Vec<ListRow> = self.transcript.iter().cloned().map(ListRow::Row).chain(tail).collect();
        let splices = diff(&self.rows, &rows);
        self.rows = rows;
        self.shown = shown;
        splices
    }

    /// The rows ChatView draws after the transcript's, in its order; none at all before the first
    /// page, and for an agent that's gone with nothing said, where the screen shows a placeholder
    /// instead of the list.
    fn tail(&self, now: u64) -> Option<Vec<ListRow>> {
        let d = &self.data;
        let running = d.info.as_ref().is_none_or(|i| i.running);
        if !d.loaded || (!running && self.transcript.is_empty()) {
            return None;
        }
        let mut out: Vec<ListRow> = Vec::new();

        let screen = d.screen.as_ref();
        let working = screen.is_some_and(|s| s.working);
        let prompt = screen.is_some_and(|s| s.prompt.is_some());
        let queue = d.info.as_ref().map_or(&[][..], |i| &i.queue[..]);
        let queued: std::collections::HashSet<String> = queue.iter().map(|q| msg_key(q)).collect();
        // one waiting in the agent's queue shows once, as a queued bubble, until the agent takes it
        let mine = d.pending.iter().filter(|p| !p.queued && !queued.contains(&msg_key(&p.text))).cloned();
        let others = others_sends(sends(d.info.as_ref()), &d.pending, now).into_iter().filter(|p| !queued.contains(&msg_key(&p.text)));
        out.extend(mine.chain(others).map(ListRow::Pending));
        out.extend(queue.iter().enumerate().map(|(index, text)| ListRow::Queued { index, text: text.clone() }));
        let held = d.info.as_ref().and_then(|i| i.held.as_deref()).unwrap_or(&[]);
        out.extend(held.iter().enumerate().map(|(index, p)| ListRow::Held { prompt: p.clone(), index, count: held.len() }));

        let live = d.live.as_deref().unwrap_or(&[]);
        let live_shown = live.iter().any(|l| l.kind != LiveKind::Output && !trim(&l.text).is_empty());
        if let Some(done) = d.catching_up {
            out.push(ListRow::CatchingUp { done });
        } else {
            if live_shown {
                out.push(ListRow::Live { items: live.to_vec() });
            }
            if working && !prompt {
                let last_open = matches!(self.transcript.last(), Some(Row::Assistant(t)) if t.open);
                // when the turn started: the agent's own record first, else read off its screen
                let since = d.info.as_ref().and_then(|i| i.turn.as_ref()).map(|t| t.started_at.0).or_else(|| screen.and_then(|s| s.since).map(|s| s as f64));
                let output = live.iter().rev().find(|l| l.kind == LiveKind::Output).map(|l| l.text.clone());
                out.push(ListRow::Working { activity: screen.and_then(|s| s.activity.clone()), since, output, joins_turn: last_open || live_shown });
            }
        }
        if !working && !prompt {
            if let Some(Some(text)) = screen.and_then(|s| s.background.as_ref()).filter(|b| b.as_ref().is_some_and(|t| !t.is_empty())) {
                out.push(ListRow::Background { text: text.clone() });
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Part;
    use crate::test_support::{chat_info, iso, item, T0};
    use serde_json::{json, Value};

    const NOW: u64 = T0 as u64;

    fn reset(key: &str, items: Vec<ChatItem>, has_more: bool) -> ServerMsg {
        reset_with(key, chat_info(json!({})), items, has_more)
    }
    fn reset_with(key: &str, info: ChatInfo, items: Vec<ChatItem>, has_more: bool) -> ServerMsg {
        ServerMsg::ChatReset { key: key.into(), info: Box::new(info), items, has_more }
    }
    fn append(items: Vec<ChatItem>) -> ServerMsg {
        ServerMsg::ChatAppend { key: "k".into(), items }
    }
    fn more(items: Vec<ChatItem>, has_more: bool) -> ServerMsg {
        ServerMsg::ChatMore { key: "k".into(), items, has_more }
    }
    fn live(items: Value) -> ServerMsg {
        ServerMsg::ChatLive { key: "k".into(), items: serde_json::from_value(items).unwrap() }
    }
    fn info(more: Value) -> ServerMsg {
        ServerMsg::ChatInfo { key: "k".into(), info: Box::new(chat_info(more)) }
    }
    fn screen(v: Value) -> ServerMsg {
        ServerMsg::ChatScreen { key: "k".into(), screen: Box::new(serde_json::from_value(v).unwrap()) }
    }
    fn lives(items: Value) -> Option<Vec<LiveItem>> {
        Some(serde_json::from_value(items).unwrap())
    }
    fn seqs(items: &[ChatItem]) -> Vec<i64> {
        items.iter().map(ChatItem::seq).collect()
    }
    /// assistant text recorded with the live message's id
    fn recorded(text: &str, id: &str) -> ChatItem {
        item::with(&item::text(text), json!({ "uuid": id, "msgId": id }))
    }

    // ---------- store.test.ts: hub messages ----------

    #[test]
    fn chat_reset_loads_the_transcript() {
        let mut c = ChatData::default();
        let items = vec![item::user("hi"), item::text("hello")];
        c.reduce(&reset_with("k", chat_info(json!({ "title": "T" })), items.clone(), true), NOW);
        assert!(c.loaded && c.has_more);
        assert_eq!(c.items, items);
        assert_eq!(c.info.unwrap().title.as_deref(), Some("T"));
    }

    #[test]
    fn live_text_is_kept_until_the_record_with_the_same_id_arrives_then_goes_with_it() {
        let mut c = ChatData::default();
        c.reduce(&reset("k", vec![item::user("say hi")], false), NOW);
        c.reduce(&live(json!([{ "id": "msg_1", "kind": "text", "text": "Hi ag" }])), NOW);
        c.reduce(&live(json!([{ "id": "msg_1", "kind": "text", "text": "Hi again!" }, { "id": "rs_1", "kind": "thinking", "text": "brief" }])), NOW);
        assert_eq!(c.live, lives(json!([{ "id": "msg_1", "kind": "text", "text": "Hi again!" }, { "id": "rs_1", "kind": "thinking", "text": "brief" }])));
        // the message lands in the record: its live copy is dropped in the same update, the other stays
        c.reduce(&append(vec![recorded("Hi again!", "msg_1")]), NOW);
        assert_eq!(c.live, lives(json!([{ "id": "rs_1", "kind": "thinking", "text": "brief" }])));
        // a live list that names something already recorded never shows it
        c.reduce(&live(json!([{ "id": "msg_1", "kind": "text", "text": "Hi again!" }])), NOW);
        assert_eq!(c.live, Some(vec![]));
        // the hub says nothing is being written
        c.reduce(&live(json!([])), NOW);
        assert_eq!(c.live, Some(vec![]));
    }

    #[test]
    fn text_read_off_the_screen_gives_way_to_a_new_reply_on_record() {
        let mut c = ChatData::default();
        c.reduce(&reset("k", vec![item::user("tea?"), item::text("An older reply.")], false), NOW);
        let tea = json!([{ "id": "screen:1", "kind": "text", "text": "Tea likely began in China" }]);
        c.reduce(&live(tea.clone()), NOW);
        // records already there (an older reply) don't hide it
        assert_eq!(c.live, lives(tea));
        // a tool call on record isn't the reply: it stays
        c.reduce(&append(vec![item::with(&item::tool("t1"), json!({ "seq": 100_050 }))]), NOW);
        assert_eq!(c.live.as_ref().unwrap().len(), 1);
        // the reply lands: the screen copy goes in the same update
        c.reduce(&append(vec![item::with(&item::text("Tea likely began in China, where..."), json!({ "seq": 100_051 }))]), NOW);
        assert_eq!(c.live, Some(vec![]));
        // what's still being written after it comes back from the hub
        let tang = json!([{ "id": "screen:1", "kind": "text", "text": "During the Tang dynasty" }]);
        c.reduce(&live(tang.clone()), NOW);
        assert_eq!(c.live, lives(tang));
        // Antigravity and Gemini send the whole conversation again: a new reply in it does the same
        let all = c.items.clone();
        c.reduce(&reset("k", all.clone(), false), NOW);
        assert_eq!(c.live.as_ref().unwrap().len(), 1);
        let mut more = all;
        more.push(item::with(&item::text("During the Tang dynasty, tea"), json!({ "seq": 100_060 })));
        c.reduce(&reset("k", more, false), NOW);
        assert_eq!(c.live, Some(vec![]));
    }

    #[test]
    fn a_resent_latest_page_keeps_the_earlier_messages_already_loaded() {
        let mut c = ChatData::default();
        let old = vec![item::user("1"), item::text("2"), item::user("3"), item::text("4")];
        c.reduce(&reset("k", old.clone(), true), NOW);
        c.reduce(&more(vec![], false), NOW);
        // the hub resends from item 3 on, with one new item
        let next = vec![old[2].clone(), old[3].clone(), item::text("5")];
        c.reduce(&reset("k", next.clone(), true), NOW);
        let mut want = seqs(&old);
        want.push(next[2].seq());
        assert_eq!(seqs(&c.items), want);
        // the reader already reached the start; that stays known
        assert!(!c.has_more);
    }

    #[test]
    fn a_page_that_starts_somewhere_unknown_replaces_everything() {
        let mut c = ChatData::default();
        c.reduce(&reset("k", vec![item::user("1"), item::text("2")], true), NOW);
        let other = vec![item::user("x"), item::text("y")];
        c.reduce(&reset("k", other.clone(), true), NOW);
        assert_eq!(c.items, other);
    }

    #[test]
    fn chat_append_adds_only_items_it_has_not_seen() {
        let mut c = ChatData::default();
        let (a, b) = (item::user("a"), item::text("b"));
        c.reduce(&reset("k", vec![a.clone(), b.clone()], false), NOW);
        let d = item::text("c");
        c.reduce(&append(vec![b.clone(), d.clone()]), NOW);
        assert_eq!(seqs(&c.items), [a.seq(), b.seq(), d.seq()]);
    }

    #[test]
    fn chat_more_prepends_older_items_without_duplicates() {
        let mut c = ChatData::default();
        let (a, b, d) = (item::user("a"), item::text("b"), item::user("c"));
        c.reduce(&reset("k", vec![b.clone(), d.clone()], true), NOW);
        c.reduce(&more(vec![a.clone(), b.clone()], false), NOW + 5);
        assert_eq!(seqs(&c.items), [a.seq(), b.seq(), d.seq()]);
        assert!(!c.has_more);
        assert_eq!(c.more_at, Some(NOW + 5));
    }

    #[test]
    fn chat_info_without_a_transcript_counts_as_loaded_with_one_it_waits_for_the_items() {
        let mut none = ChatData::default();
        none.reduce(&info(json!({})), NOW);
        assert!(none.loaded);
        let mut some = ChatData::default();
        some.reduce(&info(json!({ "transcriptPath": "/t.jsonl" })), NOW);
        assert!(!some.loaded);
        some.reduce(&reset("k", vec![], false), NOW);
        some.reduce(&info(json!({ "transcriptPath": "/t.jsonl" })), NOW);
        assert!(some.loaded);
    }

    #[test]
    fn chat_screen_and_chat_draft_land_on_the_chat_keeping_the_rest() {
        let mut c = ChatData::default();
        let items = vec![item::user("a")];
        c.reduce(&reset("k", items.clone(), false), NOW);
        c.reduce(&screen(json!({ "working": true, "width": 80 })), NOW);
        c.reduce(&ServerMsg::ChatDraft { key: "k".into(), text: "half typed".into(), source: DraftSource::Terminal, at: 5 }, NOW);
        assert_eq!(c.items, items);
        assert!(c.screen.as_ref().unwrap().working);
        assert_eq!(c.draft, Some(Draft { text: "half typed".into(), source: DraftSource::Terminal, at: 5 }));
    }

    #[test]
    fn progress_reads_as_a_fraction_and_the_next_reset_clears_it() {
        let mut c = ChatData::default();
        c.reduce(&ServerMsg::ChatProgress { key: "k".into(), done: 50, total: 200 }, NOW);
        assert_eq!(c.catching_up, Some(0.25));
        c.reduce(&ServerMsg::ChatProgress { key: "k".into(), done: 300, total: 200 }, NOW);
        assert_eq!(c.catching_up, Some(1.0));
        c.reduce(&ServerMsg::ChatProgress { key: "k".into(), done: 0, total: 0 }, NOW);
        assert_eq!(c.catching_up, None);
        c.reduce(&ServerMsg::ChatProgress { key: "k".into(), done: 1, total: 2 }, NOW);
        c.reduce(&reset("k", vec![], false), NOW);
        assert_eq!(c.catching_up, None);
    }

    // ---------- store.test.ts: pending messages ----------

    fn p(text: &str) -> PendingMessage {
        PendingMessage::new(text, text, PendingStatus::Sent, NOW)
    }
    fn texts(c: &ChatData) -> Vec<&str> {
        c.pending.iter().map(|m| m.text.as_str()).collect()
    }

    #[test]
    fn the_transcript_confirms_them_on_reset_and_on_append() {
        let ts = iso(T0 + 1000.0);
        let mut c = ChatData { pending: vec![p("one"), p("two"), p("three")], ..Default::default() };
        c.reduce(&reset("k", vec![item::user_at("one", &ts)], false), NOW);
        assert_eq!(texts(&c), ["two", "three"]);
        c.reduce(&append(vec![item::user_at("three", &ts)]), NOW);
        assert_eq!(texts(&c), ["two"]);
    }

    #[test]
    fn a_reappearing_old_item_does_not_confirm_a_new_send() {
        let old = item::user_at("yes", &iso(T0 + 1000.0));
        let mut c = ChatData::default();
        c.reduce(&reset("k", vec![old.clone()], false), NOW);
        c.pending = vec![p("yes")];
        c.reduce(&append(vec![old]), NOW);
        assert_eq!(c.pending.len(), 1);
    }

    // ---------- store.test.ts: the rest of a chat's life ----------

    #[test]
    fn a_chat_left_a_while_ago_doesnt_show_its_old_goal_when_opened_again() {
        let mut c = ChatData::default();
        let i = chat_info(json!({ "title": "T", "goal": { "condition": "ship it", "met": false, "ts": "x" }, "queue": ["later"] }));
        c.reduce(&reset_with("g", i, vec![item::user("hi")], false), NOW);
        c.forget();
        // the title stays for the header while it reloads; the goal waits for the hub to say it's still on
        assert!(c.items.is_empty() && !c.loaded);
        let info = c.info.as_ref().unwrap();
        assert_eq!(info.title.as_deref(), Some("T"));
        assert!(info.goal.is_none() && info.queue.is_empty());
    }

    #[test]
    fn a_chat_page_kept_on_the_device_shows_until_the_hub_sends_the_real_one() {
        let mut c = ChatData::default();
        c.reduce(&reset("k", vec![item::user("a"), item::text("b")], false), NOW);
        let kept = c.snapshot().unwrap();
        let mut fresh = ChatData::default();
        fresh.hydrate(kept);
        assert!(fresh.loaded && fresh.stale);
        assert_eq!(fresh.items.len(), 2);
        // a copy drawn from the device isn't kept again
        assert!(fresh.snapshot().is_none());
        fresh.reduce(&reset("k", vec![item::user("c")], false), NOW);
        assert!(!fresh.stale);
        assert_eq!(fresh.items.len(), 1);
        // a loaded chat isn't painted over
        fresh.hydrate(ChatSnapshot { info: None, items: vec![], has_more: false });
        assert_eq!(fresh.items.len(), 1);
    }

    #[test]
    fn a_kept_page_holds_the_last_40_messages() {
        let mut c = ChatData::default();
        let items: Vec<ChatItem> = (0..45).map(|i| item::user(&format!("m{i}"))).collect();
        c.reduce(&reset("k", items.clone(), false), NOW);
        let kept = c.snapshot().unwrap();
        assert_eq!(kept.items, items[5..]);
        assert!(kept.has_more);
    }

    // ---------- the list and its splices ----------

    /// A chat whose every change is checked: the splices turn the old rows into the new ones.
    struct Watched {
        chat: Chat,
        mirror: Vec<ListRow>,
    }

    impl Watched {
        fn new() -> Self {
            Watched { chat: Chat::new(), mirror: Vec::new() }
        }
        fn check(&mut self, splices: Vec<Splice>) -> Vec<Splice> {
            apply_splices(&mut self.mirror, &splices);
            assert_eq!(self.mirror, self.chat.rows(), "the splices rebuild the list");
            splices
        }
        fn apply(&mut self, msg: ServerMsg) -> Vec<Splice> {
            let s = self.chat.apply(&msg, NOW);
            self.check(s)
        }
        fn patch(&mut self, f: impl FnOnce(&mut ChatData)) -> Vec<Splice> {
            let s = self.chat.patch(NOW, f);
            self.check(s)
        }
        fn tick(&mut self, now: u64) -> Vec<Splice> {
            let s = self.chat.tick(now);
            self.check(s)
        }
        fn keys(&self) -> Vec<String> {
            self.chat.rows().iter().map(|r| r.key().into_owned()).collect()
        }
    }

    fn shape(splices: &[Splice]) -> Vec<String> {
        splices
            .iter()
            .map(|s| match s {
                Splice::Insert { at, rows } => format!("insert {at} {}", rows.iter().map(|r| r.key().into_owned()).collect::<Vec<_>>().join(",")),
                Splice::Replace { at, row } => format!("replace {at} {}", row.key()),
                Splice::Remove { at, count } => format!("remove {at} {count}"),
            })
            .collect()
    }

    #[test]
    fn nothing_shows_before_the_first_page_then_it_comes_in_one_insert() {
        let mut w = Watched::new();
        assert!(w.apply(info(json!({ "transcriptPath": "/t.jsonl" }))).is_empty());
        let (u, a) = (item::user("hi"), item::text("hello"));
        let s = w.apply(reset("k", vec![u.clone(), a.clone()], false));
        assert_eq!(shape(&s), [format!("insert 0 u{},a{}", u.seq(), a.seq())]);
    }

    #[test]
    fn a_new_message_is_appended_and_a_growing_turn_is_replaced_in_place() {
        let mut w = Watched::new();
        let (u, a) = (item::user("hi"), item::text("one"));
        w.apply(reset("k", vec![u, a.clone()], false));
        // the reply goes on: same turn, one more part
        let s = w.apply(append(vec![item::tool("t1")]));
        assert_eq!(shape(&s), [format!("replace 1 a{}", a.seq())]);
        // its result fills in the call
        let s = w.apply(append(vec![item::result("t1", "out")]));
        assert_eq!(shape(&s), [format!("replace 1 a{}", a.seq())]);
        // a new message from you
        let u2 = item::user("next");
        let s = w.apply(append(vec![item::end(10.0), u2.clone()]));
        assert_eq!(shape(&s), [format!("replace 1 a{}", a.seq()), format!("insert 2 u{}", u2.seq())]);
        // the same items again change nothing
        assert!(w.apply(append(vec![u2])).is_empty());
    }

    #[test]
    fn a_streaming_reply_is_replaced_as_it_grows_and_the_record_takes_its_place() {
        let mut w = Watched::new();
        let u = item::user("say hi");
        w.apply(reset("k", vec![u.clone()], false));
        let s = w.apply(live(json!([{ "id": "msg_1", "kind": "text", "text": "Hi" }])));
        assert_eq!(shape(&s), ["insert 1 live"]);
        let s = w.apply(live(json!([{ "id": "msg_1", "kind": "text", "text": "Hi again" }])));
        assert_eq!(shape(&s), ["replace 1 live"]);
        // the record lands: the live row goes and the turn comes in its place, in one update
        let rec = recorded("Hi again!", "msg_1");
        let s = w.apply(append(vec![rec.clone()]));
        assert_eq!(shape(&s), ["remove 1 1".to_string(), format!("insert 1 a{}", rec.seq())]);
        assert_eq!(w.keys(), [format!("u{}", u.seq()), format!("a{}", rec.seq())]);
    }

    #[test]
    fn a_screen_streamed_bubble_retires_when_the_transcript_row_lands() {
        let mut w = Watched::new();
        let (u, old) = (item::user("tea?"), item::text("An older reply."));
        w.apply(reset("k", vec![u, old.clone()], false));
        w.apply(screen(json!({ "working": true, "width": 80, "activity": "Brewing…" })));
        let s = w.apply(live(json!([{ "id": "screen:1", "kind": "text", "text": "Tea likely began" }])));
        assert_eq!(shape(&s), ["insert 2 live"]);
        // a tool call on record isn't the reply: the screen text stays
        let s = w.apply(append(vec![item::with(&item::tool("t1"), json!({ "seq": 200_050 }))]));
        assert_eq!(shape(&s), [format!("replace 1 a{}", old.seq())]);
        // the reply lands: the screen copy goes with the same update
        let s = w.apply(append(vec![item::with(&item::text("Tea likely began in China"), json!({ "seq": 200_051 }))]));
        assert_eq!(shape(&s), [format!("replace 1 a{}", old.seq()), "remove 2 1".into()]);
        let Some(ListRow::Row(Row::Assistant(t))) = w.chat.rows().get(1) else { panic!() };
        assert!(matches!(t.parts.last(), Some(Part::Text { text, .. }) if text == "Tea likely began in China"));
    }

    #[test]
    fn the_working_line_follows_the_screen_and_goes_when_the_agent_stops() {
        let mut w = Watched::new();
        w.apply(reset("k", vec![item::user("go"), item::text("on it")], false));
        let s = w.apply(screen(json!({ "working": true, "width": 80, "activity": "Thinking… (1s)" })));
        assert_eq!(shape(&s), ["insert 2 working"]);
        assert!(matches!(&w.chat.rows()[2], ListRow::Working { activity: Some(a), joins_turn: true, .. } if a == "Thinking… (1s)"));
        let s = w.apply(screen(json!({ "working": true, "width": 80, "activity": "Thinking… (2s)" })));
        assert_eq!(shape(&s), ["replace 2 working"]);
        // the same screen again changes nothing
        assert!(w.apply(screen(json!({ "working": true, "width": 80, "activity": "Thinking… (2s)" }))).is_empty());
        // a question on screen hides it
        let s = w.apply(screen(json!({ "working": true, "width": 80, "prompt": { "body": ["Allow?"], "options": [], "multi": false } })));
        assert_eq!(shape(&s), ["remove 2 1"]);
        let s = w.apply(screen(json!({ "working": false, "width": 80, "background": "1 shell" })));
        assert_eq!(shape(&s), ["insert 2 background"]);
        let s = w.apply(screen(json!({ "working": false, "width": 80, "background": null })));
        assert_eq!(shape(&s), ["remove 2 1"]);
    }

    #[test]
    fn a_sent_bubble_retires_when_its_message_lands() {
        let mut w = Watched::new();
        let a = item::text("ready");
        w.apply(reset("k", vec![a.clone()], false));
        let s = w.patch(|c| c.pending.push(p("fix the bug")));
        assert_eq!(shape(&s), ["insert 1 pfix the bug"]);
        let u = item::user_at("fix the bug", &iso(T0 + 1000.0));
        let s = w.apply(append(vec![u.clone()]));
        // the turn above ends with it
        assert_eq!(shape(&s), [format!("replace 0 a{}", a.seq()), "remove 1 1".into(), format!("insert 1 u{}", u.seq())]);
        assert!(w.chat.pending.is_empty());
    }

    #[test]
    fn the_hubs_ledger_moves_a_bubble_to_the_queue_and_drops_it_once_delivered() {
        let mut w = Watched::new();
        w.apply(reset("k", vec![item::user("start")], false));
        w.patch(|c| c.pending.push(p("next")));
        let ledger = |state: &str| json!({ "sends": [{ "id": "next", "text": "next", "at": NOW, "state": state }] });
        // the agent queued it: one bubble, in the queue's place
        let mut queued = ledger("queued");
        queued["queue"] = json!(["next"]);
        let s = w.apply(info(queued));
        assert_eq!(shape(&s), ["remove 1 1", "insert 1 q0:next"]);
        assert!(w.chat.pending[0].queued);
        // delivered: gone from the list once the queue lets it go
        let s = w.apply(info(ledger("delivered")));
        assert_eq!(shape(&s), ["remove 1 1"]);
        assert!(w.chat.pending.is_empty());
    }

    #[test]
    fn a_send_from_another_device_shows_until_it_arrives_or_grows_old() {
        let mut w = Watched::new();
        w.apply(reset("k", vec![item::user("start")], false));
        let s = w.apply(info(json!({ "sends": [{ "id": "o1", "text": "from the laptop", "at": NOW, "state": "sending" }] })));
        assert_eq!(shape(&s), ["insert 1 po1"]);
        assert!(matches!(&w.chat.rows()[1], ListRow::Pending(m) if m.remote));
        assert!(w.tick(NOW + 60_000).is_empty());
        assert_eq!(shape(&w.tick(NOW + 5 * 60_000)), ["remove 1 1"]);
    }

    #[test]
    fn a_send_that_never_lands_fails_after_ten_minutes() {
        let mut w = Watched::new();
        w.apply(reset("k", vec![item::user("start")], false));
        w.patch(|c| c.pending.push(p("hello?")));
        assert!(w.tick(NOW + PENDING_MS - 1).is_empty());
        let s = w.tick(NOW + PENDING_MS);
        assert_eq!(shape(&s), ["replace 1 phello?"]);
        assert!(matches!(&w.chat.rows()[1], ListRow::Pending(m) if m.status == PendingStatus::Failed && m.error.as_deref().is_some_and(|e| e.contains("10 minutes"))));
    }

    #[test]
    fn earlier_messages_come_in_at_the_top_and_a_turn_they_join_takes_its_new_key() {
        let mut w = Watched::new();
        let (u1, a1, a2, u2) = (item::user("q"), item::text("part one"), item::text("part two"), item::user("next"));
        w.apply(reset("k", vec![a2.clone(), u2.clone()], true));
        assert_eq!(w.keys(), [format!("a{}", a2.seq()), format!("u{}", u2.seq())]);
        // the page before ends in the same turn: it starts earlier now
        let s = w.apply(more(vec![u1.clone(), a1.clone()], false));
        assert_eq!(shape(&s), ["remove 0 1".to_string(), format!("insert 0 u{},a{}", u1.seq(), a1.seq())]);
    }

    #[test]
    fn queue_held_messages_catching_up_and_live_text_keep_chatviews_order() {
        let mut w = Watched::new();
        w.apply(reset("k", vec![item::user("go")], false));
        w.patch(|c| c.pending.push(p("mine")));
        w.apply(info(json!({ "queue": ["one", "two"], "held": [{ "id": "h1", "text": "held", "at": 1 }] })));
        w.apply(live(json!([{ "id": "x", "kind": "text", "text": "writing" }, { "id": "o", "kind": "output", "text": "$ make" }])));
        w.apply(screen(json!({ "working": true, "width": 80 })));
        let keys: Vec<String> = w.keys().into_iter().skip(1).collect();
        assert_eq!(keys, ["pmine", "q0:one", "q1:two", "hh1", "live", "working"]);
        assert!(matches!(w.chat.rows().last(), Some(ListRow::Working { output: Some(o), .. }) if o == "$ make"));
        // catching up replaces the live text and the working line
        let s = w.apply(ServerMsg::ChatProgress { key: "k".into(), done: 1, total: 4 });
        assert_eq!(shape(&s), ["remove 5 2", "insert 5 reading"]);
        // output alone isn't live text: the working line keeps it and no longer sits under live text
        w.apply(ServerMsg::ChatProgress { key: "k".into(), done: 0, total: 0 });
        let s = w.apply(live(json!([{ "id": "o", "kind": "output", "text": "$ make" }])));
        assert_eq!(shape(&s), ["remove 5 1", "replace 5 working"]);
    }

    #[test]
    fn a_gone_agent_with_nothing_said_shows_no_list_and_forgetting_empties_it() {
        let mut w = Watched::new();
        w.apply(reset_with("k", chat_info(json!({ "running": false })), vec![], false));
        assert!(w.chat.rows().is_empty());
        w.apply(reset("k", vec![item::user("a")], false));
        assert_eq!(w.chat.rows().len(), 1);
        let s = w.chat.forget(NOW);
        let s = w.check(s);
        assert_eq!(shape(&s), ["remove 0 1"]);
    }

    /// A small deterministic generator, so the randomized test needs no crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn any_run_of_messages_yields_splices_that_rebuild_the_list() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let mut w = Watched::new();
            w.apply(reset("k", vec![item::user("start")], false));
            for step in 0..60 {
                let msg = match rng.next(9) {
                    0 => append(vec![item::user(&format!("u{step}"))]),
                    1 => append(vec![item::text(&format!("t{step}"))]),
                    2 => append(vec![item::tool(&format!("c{step}")), item::result(&format!("c{}", step.max(1) - 1), "ok")]),
                    3 => append(vec![item::end(5.0)]),
                    4 => live(json!([{ "id": format!("l{}", rng.next(3)), "kind": "text", "text": format!("w{step}") }])),
                    5 => screen(json!({ "working": rng.next(2) == 0, "width": 80, "activity": format!("s{}", rng.next(3)) })),
                    6 => info(json!({ "queue": (0..rng.next(3)).map(|i| format!("q{i}")).collect::<Vec<_>>() })),
                    7 => {
                        let s = w.chat.patch(NOW, |c| c.pending.push(PendingMessage::new(format!("p{step}"), format!("p{step}"), PendingStatus::Sent, NOW)));
                        w.check(s);
                        append(vec![item::user_at(&format!("p{}", step.max(1) - 1), &iso(T0 + 1000.0))])
                    }
                    _ => {
                        let first = w.chat.items.first().map_or(0, ChatItem::seq);
                        more(vec![item::with(&item::text("older"), json!({ "seq": first - 1 - rng.next(5) as i64 }))], rng.next(2) == 0)
                    }
                };
                w.apply(msg);
            }
        }
    }

    #[test]
    fn diff_handles_the_edges() {
        let row = |k: &str| ListRow::Pending(PendingMessage::new(k, k, PendingStatus::Sent, 0));
        let changed = |k: &str| ListRow::Pending(PendingMessage::new(k, "other", PendingStatus::Sent, 0));
        let check = |old: Vec<ListRow>, new: Vec<ListRow>| {
            let s = diff(&old, &new);
            let mut m = old;
            apply_splices(&mut m, &s);
            assert_eq!(m, new);
            s.len()
        };
        assert_eq!(check(vec![], vec![]), 0);
        assert_eq!(check(vec![row("a")], vec![]), 1);
        assert_eq!(check(vec![], vec![row("a"), row("b")]), 1);
        assert_eq!(check(vec![row("a"), row("b"), row("c")], vec![row("a"), changed("b"), row("c")]), 1);
        assert_eq!(check(vec![row("a"), row("b")], vec![row("b"), row("a")]), 2);
        assert_eq!(check(vec![row("a"), row("x"), row("c")], vec![row("a"), row("y"), changed("c")]), 3);
    }
}
