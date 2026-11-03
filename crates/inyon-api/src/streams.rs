//! Protocol v2 on `/ws2` (described in inyon-protocol's `v2.md`): the hub's streams and the apps
//! that follow them.
//!
//! A stream is the overview, one chat, how the sessions are organised, or the pages. It keeps its state as
//! rows with keys, and for each row
//! the `seq` it was added and last changed at, plus the keys of rows that went. That is all it
//! needs to bring an app from any `seq` of the current epoch to now in one delta, so it keeps no log
//! of frames. A chat stream follows the chat's watcher like a v1 app does and turns the watcher's
//! messages into rows: a resent page of 160 messages becomes the few rows that changed, and the
//! live reply becomes one row replaced in place.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::rc::Rc;

use indexmap::IndexMap;
use inyon_core::claude::watcher::{watchers, Sub, Subscriber};
use inyon_core::clock::now_ms;
use inyon_core::hosts::registry::registry;
use inyon_core::platform::rt::spawn;
use inyon_protocol::v2::{page_key, split_queue, ORGANIZE, OVERVIEW, PAGES, PROTOCOL_V2, ZIP_MIN};
use inyon_protocol::{
    AssistantBlock, ChatInfo, ChatItem, ChatState, DelayedSend, Delta, DraftSource, DraftState, HostState, Keys, LiveItem, Organize, PageEntry, Progress, Project, ProjectGroup,
    QueuedMsg, Rows, SavedPrompt, ScreenState, ServerMsg, ServerMsg2, SessionMark, SleepRule, Snapshot, Space, StreamPos, Zip,
};
use serde_json::Value;

use crate::clients::{all, quiet_key, raw, s, when_pane, ClientRef};

/// A stream nobody follows is kept this long, so an app that comes back gets a delta.
const KEEP_MS: u64 = 30 * 60_000;
/// streams kept at most, followed or not
const MAX_STREAMS: usize = 256;
/// chats one app can follow at once
const MAX_CHATS: usize = 64;
/// how many removed rows a collection remembers; an app further behind gets a snapshot
const GONE: usize = 2048;
/// transcript items a chat stream holds; older ones stay with the apps, which page further back
const ITEMS: usize = 1000;

// ---------- rows ----------

/// A row of a collection, and the key it is found by.
pub(crate) trait Row: Clone + PartialEq {
    type Key: Clone + Eq + Hash;
    fn key(&self) -> Self::Key;
}

impl Row for HostState {
    type Key = String;
    fn key(&self) -> String {
        self.config.id.clone()
    }
}

impl Row for ChatItem {
    type Key = i64;
    fn key(&self) -> i64 {
        self.seq()
    }
}

impl Row for LiveItem {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

impl Row for QueuedMsg {
    type Key = String;
    fn key(&self) -> String {
        QueuedMsg::key(self)
    }
}

macro_rules! row_by_id {
    ($($t:ty),*) => {$(
        impl Row for $t {
            type Key = String;
            fn key(&self) -> String {
                self.id.clone()
            }
        }
    )*};
}
row_by_id!(SessionMark, Project, ProjectGroup, Space, DelayedSend, SavedPrompt);

impl Row for PageEntry {
    type Key = String;
    fn key(&self) -> String {
        page_key(self)
    }
}

impl Row for SleepRule {
    type Key = String;
    fn key(&self) -> String {
        self.scope.clone()
    }
}

struct Entry<V> {
    v: V,
    /// when the key first came in this epoch (kept while it is away and back)
    first: u64,
    /// when it was last appended
    added: u64,
    changed: u64,
}

struct Gone<K> {
    key: K,
    first: u64,
    at: u64,
}

/// What changes one collection: applied as remove, replace, append.
pub(crate) struct Ops<V: Row> {
    pub remove: Vec<V::Key>,
    pub replace: Vec<V>,
    pub append: Vec<V>,
}

impl<V: Row> Default for Ops<V> {
    fn default() -> Self {
        Ops { remove: vec![], replace: vec![], append: vec![] }
    }
}

/// One collection of a stream, in order, with what it takes to tell an app what changed since any
/// `seq` of the epoch.
pub(crate) struct Coll<V: Row> {
    rows: Vec<Entry<V>>,
    gone: VecDeque<Gone<V::Key>>,
    /// an app from before this seq can't be told every row that went: it gets a snapshot
    floor: u64,
    cap: usize,
}

impl<V: Row> Coll<V> {
    pub fn new(cap: usize) -> Self {
        Coll { rows: Vec::new(), gone: VecDeque::new(), floor: 0, cap }
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + '_ {
        self.rows.iter().map(|e| &e.v)
    }

    fn find(&self, k: &V::Key) -> Option<usize> {
        // the rows that change are nearly always the latest ones
        self.rows.iter().rposition(|e| e.v.key() == *k)
    }

    pub fn get(&self, k: &V::Key) -> Option<&V> {
        self.find(k).map(|i| &self.rows[i].v)
    }

    /// Starts over at `seq` (a new epoch): every row as if it had just come.
    pub fn restamp(&mut self, seq: u64) {
        for e in &mut self.rows {
            (e.first, e.added, e.changed) = (seq, seq, seq);
        }
        self.gone.clear();
        self.floor = seq;
    }

    /// Makes the rows `new`, and says how: the longest start of `new` already here in the same
    /// order stays (replaced where it changed), the rest of the old rows go, and the rest of `new`
    /// is appended. Applied as remove, replace, append, that ends with exactly `new`.
    pub fn set(&mut self, new: Vec<V>, seq: u64) -> Ops<V> {
        let mut seen = HashSet::new();
        let new: Vec<V> = new.into_iter().filter(|v| seen.insert(v.key())).collect();
        let at: HashMap<V::Key, usize> = self.rows.iter().enumerate().map(|(i, e)| (e.v.key(), i)).collect();
        let mut last: Option<usize> = None;
        let mut i = 0;
        while i < new.len() {
            match at.get(&new[i].key()) {
                Some(&j) if last.is_none_or(|l| j > l) => {
                    last = Some(j);
                    i += 1;
                }
                _ => break,
            }
        }
        let kept: HashSet<V::Key> = new[..i].iter().map(Row::key).collect();
        let ops = Ops {
            remove: self.rows.iter().map(|e| e.v.key()).filter(|k| !kept.contains(k)).collect(),
            replace: new[..i].iter().filter(|v| self.rows[at[&v.key()]].v != **v).cloned().collect(),
            append: new[i..].to_vec(),
        };
        self.apply(&ops, seq);
        ops
    }

    /// Adds rows at the end; one whose key is already here replaces it in place.
    pub fn push(&mut self, rows: Vec<V>, seq: u64) -> Ops<V> {
        let mut ops = Ops::default();
        for v in rows {
            match self.find(&v.key()) {
                Some(i) if self.rows[i].v == v => {}
                Some(_) => ops.replace.push(v),
                None => ops.append.push(v),
            }
        }
        self.apply(&ops, seq);
        ops
    }

    pub fn apply(&mut self, ops: &Ops<V>, seq: u64) {
        for k in &ops.remove {
            if let Some(i) = self.find(k) {
                let e = self.rows.remove(i);
                self.gone.push_back(Gone { key: k.clone(), first: e.first, at: seq });
                if self.gone.len() > self.cap {
                    if let Some(g) = self.gone.pop_front() {
                        self.floor = self.floor.max(g.at);
                    }
                }
            }
        }
        for v in &ops.replace {
            if let Some(i) = self.find(&v.key()) {
                let e = &mut self.rows[i];
                e.v = v.clone();
                e.changed = seq;
            }
        }
        for v in &ops.append {
            let k = v.key();
            if let Some(i) = self.find(&k) {
                self.rows.remove(i);
            }
            let first = match self.gone.iter().rposition(|g| g.key == k) {
                Some(g) => self.gone.remove(g).map_or(seq, |g| g.first),
                None => seq,
            };
            self.rows.push(Entry { v: v.clone(), first, added: seq, changed: seq });
        }
    }

    /// Lets the oldest rows go without telling anyone (the apps keep them as history). Whether any went.
    pub fn forget_oldest(&mut self, keep: usize) -> bool {
        let n = self.rows.len().saturating_sub(keep);
        self.rows.drain(..n);
        n > 0
    }

    /// What changed since `from`, as one set of ops; None when the collection no longer knows.
    /// Rows appended since are all at the end, in order: each went after every row there then.
    pub fn since(&self, from: u64) -> Option<Ops<V>> {
        if from < self.floor {
            return None;
        }
        let mut ops = Ops::default();
        // gone since, and there before: a row that came and went since is nothing to the app
        ops.remove.extend(self.gone.iter().filter(|g| g.at > from && g.first <= from).map(|g| g.key.clone()));
        for e in &self.rows {
            if e.added > from {
                // there before and moved since: out of its old place first
                if e.first <= from {
                    ops.remove.push(e.v.key());
                }
                ops.append.push(e.v.clone());
            } else if e.changed > from {
                ops.replace.push(e.v.clone());
            }
        }
        Some(ops)
    }
}

/// A single value of a stream, and when it last changed.
struct Field<T> {
    v: Option<T>,
    changed: u64,
}

impl<T: PartialEq> Field<T> {
    fn new() -> Self {
        Field { v: None, changed: 0 }
    }

    /// Whether it changed.
    fn set(&mut self, v: T, seq: u64) -> bool {
        if self.v.as_ref() == Some(&v) {
            return false;
        }
        self.v = Some(v);
        self.changed = seq;
        true
    }

    fn since(&self, from: u64) -> bool {
        self.v.is_some() && self.changed > from
    }
}

// ---------- the streams ----------

struct ChatBody {
    items: Coll<ChatItem>,
    live: Coll<LiveItem>,
    queue: Coll<QueuedMsg>,
    info: Field<Box<ChatInfo>>,
    screen: Field<Box<ScreenState>>,
    draft: Field<DraftState>,
    progress: Field<Progress>,
    has_more: Field<bool>,
    loaded: Field<bool>,
}

impl ChatBody {
    fn new(gone: usize) -> Self {
        ChatBody {
            items: Coll::new(gone),
            live: Coll::new(gone),
            queue: Coll::new(gone),
            info: Field::new(),
            screen: Field::new(),
            draft: Field::new(),
            progress: Field::new(),
            has_more: Field::new(),
            loaded: Field::new(),
        }
    }

    fn loaded(&self) -> bool {
        self.loaded.v == Some(true)
    }

    fn state_since(&self, from: u64) -> ChatState {
        ChatState {
            info: self.info.since(from).then(|| self.info.v.clone()).flatten(),
            screen: self.screen.since(from).then(|| self.screen.v.clone()).flatten(),
            draft: self.draft.since(from).then(|| self.draft.v.clone()).flatten(),
            progress: self.progress.since(from).then_some(self.progress.v).flatten(),
            has_more: self.has_more.since(from).then_some(self.has_more.v).flatten(),
            loaded: self.loaded.since(from).then_some(self.loaded.v).flatten(),
        }
    }

    fn restamp(&mut self, seq: u64) {
        self.items.restamp(seq);
        self.live.restamp(seq);
        self.queue.restamp(seq);
        for c in [&mut self.info.changed, &mut self.screen.changed, &mut self.draft.changed, &mut self.progress.changed, &mut self.has_more.changed, &mut self.loaded.changed] {
            *c = seq;
        }
    }
}

struct OrganizeBody {
    sessions: Coll<SessionMark>,
    projects: Coll<Project>,
    groups: Coll<ProjectGroup>,
    spaces: Coll<Space>,
    sends: Coll<DelayedSend>,
    prompts: Coll<SavedPrompt>,
    sleep: Coll<SleepRule>,
}

/// A change to each of the organize stream's collections.
#[derive(Default)]
struct OrganizeOps {
    sessions: Ops<SessionMark>,
    projects: Ops<Project>,
    groups: Ops<ProjectGroup>,
    spaces: Ops<Space>,
    sends: Ops<DelayedSend>,
    prompts: Ops<SavedPrompt>,
    sleep: Ops<SleepRule>,
}

impl OrganizeBody {
    fn new(gone: usize) -> Self {
        OrganizeBody {
            sessions: Coll::new(gone),
            projects: Coll::new(gone),
            groups: Coll::new(gone),
            spaces: Coll::new(gone),
            sends: Coll::new(gone),
            prompts: Coll::new(gone),
            sleep: Coll::new(gone),
        }
    }

    fn restamp(&mut self, seq: u64) {
        self.sessions.restamp(seq);
        self.projects.restamp(seq);
        self.groups.restamp(seq);
        self.spaces.restamp(seq);
        self.sends.restamp(seq);
        self.prompts.restamp(seq);
        self.sleep.restamp(seq);
    }

    fn set(&mut self, org: Organize, seq: u64) -> OrganizeOps {
        OrganizeOps {
            sessions: self.sessions.set(org.sessions, seq),
            projects: self.projects.set(org.projects, seq),
            groups: self.groups.set(org.groups, seq),
            spaces: self.spaces.set(org.spaces, seq),
            sends: self.sends.set(org.sends.unwrap_or_default(), seq),
            prompts: self.prompts.set(org.prompts.unwrap_or_default(), seq),
            sleep: self.sleep.set(org.sleep.unwrap_or_default(), seq),
        }
    }

    /// What changed after `from`, if the stream still remembers that far back.
    fn since(&self, from: u64) -> Option<OrganizeOps> {
        Some(OrganizeOps {
            sessions: self.sessions.since(from)?,
            projects: self.projects.since(from)?,
            groups: self.groups.since(from)?,
            spaces: self.spaces.since(from)?,
            sends: self.sends.since(from)?,
            prompts: self.prompts.since(from)?,
            sleep: self.sleep.since(from)?,
        })
    }

    fn rows(&self) -> Rows {
        Rows {
            sessions: Some(self.sessions.values().cloned().collect()),
            projects: Some(self.projects.values().cloned().collect()),
            groups: Some(self.groups.values().cloned().collect()),
            spaces: Some(self.spaces.values().cloned().collect()),
            sends: Some(self.sends.values().cloned().collect()),
            prompts: Some(self.prompts.values().cloned().collect()),
            sleep: Some(self.sleep.values().cloned().collect()),
            ..Rows::default()
        }
    }
}

enum Body {
    Overview(Coll<HostState>),
    Chat(Box<ChatBody>),
    Organize(Box<OrganizeBody>),
    Pages(Coll<PageEntry>),
}

/// One change to a stream, gathered as the parts of a delta.
#[derive(Default)]
struct Change {
    remove: Keys,
    replace: Rows,
    append: Rows,
    state: ChatState,
}

fn put<T>(to: &mut Option<Vec<T>>, more: Vec<T>) {
    if !more.is_empty() {
        to.get_or_insert_with(Vec::new).extend(more);
    }
}

/// One collection's change, into a delta's parts.
fn put_ops<V: Row>(remove: &mut Option<Vec<V::Key>>, replace: &mut Option<Vec<V>>, append: &mut Option<Vec<V>>, o: Ops<V>) {
    put(remove, o.remove);
    put(replace, o.replace);
    put(append, o.append);
}

impl Change {
    fn hosts(&mut self, o: Ops<HostState>) {
        put(&mut self.remove.hosts, o.remove);
        put(&mut self.replace.hosts, o.replace);
        put(&mut self.append.hosts, o.append);
    }
    fn items(&mut self, o: Ops<ChatItem>) {
        put(&mut self.remove.items, o.remove);
        put(&mut self.replace.items, o.replace);
        put(&mut self.append.items, o.append);
    }
    fn live(&mut self, o: Ops<LiveItem>) {
        put(&mut self.remove.live, o.remove);
        put(&mut self.replace.live, o.replace);
        put(&mut self.append.live, o.append);
    }
    fn queue(&mut self, o: Ops<QueuedMsg>) {
        put(&mut self.remove.queue, o.remove);
        put(&mut self.replace.queue, o.replace);
        put(&mut self.append.queue, o.append);
    }
    fn pages(&mut self, o: Ops<PageEntry>) {
        put_ops(&mut self.remove.pages, &mut self.replace.pages, &mut self.append.pages, o);
    }
    fn organize(&mut self, o: OrganizeOps) {
        let (rm, rp, ap) = (&mut self.remove, &mut self.replace, &mut self.append);
        put_ops(&mut rm.sessions, &mut rp.sessions, &mut ap.sessions, o.sessions);
        put_ops(&mut rm.projects, &mut rp.projects, &mut ap.projects, o.projects);
        put_ops(&mut rm.groups, &mut rp.groups, &mut ap.groups, o.groups);
        put_ops(&mut rm.spaces, &mut rp.spaces, &mut ap.spaces, o.spaces);
        put_ops(&mut rm.sends, &mut rp.sends, &mut ap.sends, o.sends);
        put_ops(&mut rm.prompts, &mut rp.prompts, &mut ap.prompts, o.prompts);
        put_ops(&mut rm.sleep, &mut rp.sleep, &mut ap.sleep, o.sleep);
    }

    fn into_delta(self, stream: &str, epoch: u64, from: u64, seq: u64) -> Delta {
        let some_keys = |k: Keys| (k != Keys::default()).then_some(k);
        let some_rows = |r: Rows| (r != Rows::default()).then_some(r);
        Delta {
            stream: stream.to_owned(),
            epoch,
            from,
            seq,
            remove: some_keys(self.remove),
            replace: some_rows(self.replace),
            append: some_rows(self.append),
            state: (!self.state.is_empty()).then_some(self.state),
        }
    }
}

/// What a change to a stream comes to.
enum Out {
    Nothing,
    /// one delta for everyone in step
    Delta(Box<Delta>),
    /// the history was rebuilt in a new epoch: everyone gets a snapshot
    Rebuilt,
}

pub(crate) struct Stream {
    name: String,
    epoch: u64,
    seq: u64,
    /// it has had data: until then an app that needs a snapshot waits for it
    ready: bool,
    body: Body,
    /// a chat's watcher feed, subscribed while apps follow it
    feed: Option<Sub>,
    /// when the last app stopped following it
    left_at: Option<u64>,
}

thread_local! {
    static STREAMS: RefCell<HashMap<String, Stream>> = RefCell::new(HashMap::new());
    static LAST_EPOCH: Cell<u64> = const { Cell::new(0) };
    /// removed rows a collection remembers (tests make it small)
    static GONE_CAP: Cell<usize> = const { Cell::new(GONE) };
}

/// A new epoch: from the clock, so a restarted hub's are always later, and never the same twice.
fn next_epoch() -> u64 {
    LAST_EPOCH.with(|l| {
        let e = now_ms().max(l.get() + 1);
        l.set(e);
        e
    })
}

impl Stream {
    fn new(name: &str) -> Self {
        let gone = GONE_CAP.with(Cell::get);
        let body = match name {
            OVERVIEW => Body::Overview(Coll::new(gone)),
            ORGANIZE => Body::Organize(Box::new(OrganizeBody::new(gone))),
            PAGES => Body::Pages(Coll::new(gone)),
            _ => Body::Chat(Box::new(ChatBody::new(gone))),
        };
        Stream { name: name.to_owned(), epoch: next_epoch(), seq: 0, ready: false, body, feed: None, left_at: None }
    }

    fn pos(&self) -> StreamPos {
        StreamPos { epoch: self.epoch, seq: self.seq }
    }

    /// A new epoch at the next seq, everything as it is now.
    fn rebuild(&mut self) -> Out {
        self.epoch = next_epoch();
        self.seq += 1;
        self.ready = true;
        match &mut self.body {
            Body::Overview(hosts) => hosts.restamp(self.seq),
            Body::Chat(c) => c.restamp(self.seq),
            Body::Organize(o) => o.restamp(self.seq),
            Body::Pages(pages) => pages.restamp(self.seq),
        }
        Out::Rebuilt
    }

    fn commit(&mut self, ch: Change) -> Out {
        if !self.ready {
            return self.rebuild();
        }
        let d = ch.into_delta(&self.name, self.epoch, self.seq, self.seq + 1);
        if d.is_empty() {
            return Out::Nothing;
        }
        self.seq += 1;
        Out::Delta(Box::new(d))
    }

    fn on_hosts(&mut self, hosts: Vec<HostState>) -> Out {
        let seq = self.seq + 1;
        let Body::Overview(coll) = &mut self.body else { return Out::Nothing };
        let mut ch = Change::default();
        ch.hosts(coll.set(hosts, seq));
        self.commit(ch)
    }

    fn on_pages(&mut self, pages: Vec<PageEntry>) -> Out {
        let seq = self.seq + 1;
        let Body::Pages(coll) = &mut self.body else { return Out::Nothing };
        let mut ch = Change::default();
        ch.pages(coll.set(pages, seq));
        self.commit(ch)
    }

    fn on_organize(&mut self, org: Organize) -> Out {
        let seq = self.seq + 1;
        let Body::Organize(o) = &mut self.body else { return Out::Nothing };
        let mut ch = Change::default();
        ch.organize(o.set(org, seq));
        self.commit(ch)
    }

    /// One of the watcher's messages, as a v1 app would get it.
    fn on_chat(&mut self, msg: &ServerMsg) -> Out {
        let seq = self.seq + 1;
        let ready = self.ready;
        let Body::Chat(c) = &mut self.body else { return Out::Nothing };
        let mut ch = Change::default();
        match msg {
            ServerMsg::ChatReset { info, items, has_more, .. } => {
                let loaded = c.loaded();
                // as the app does: a page that starts on a message already here (Gemini and OpenCode
                // resend their latest page on every step) keeps the earlier ones
                let keep = items.first().is_some_and(|head| *has_more && c.items.get(&head.seq()).is_some_and(|o| o.uuid() == head.uuid()));
                let oldest = c.items.values().next().map(ChatItem::seq);
                let next: Vec<ChatItem> = match items.first() {
                    Some(head) if keep => c.items.values().filter(|i| i.seq() < head.seq()).cloned().chain(items.iter().cloned()).collect(),
                    // what's older than the stream holds stays with the apps that paged back to it
                    _ => items.iter().filter(|i| oldest.is_none_or(|o| i.seq() >= o)).cloned().collect(),
                };
                // The apps may hold messages older than the stream's oldest. A delta keeps them, so it
                // only fits when the page still starts at the stream's oldest message and every place
                // holds the message it held. Otherwise (a conversation cleared, rewritten into a new
                // record, or a page that starts later than what's here) everyone starts over.
                let rewritten =
                    next.iter().any(|i| c.items.get(&i.seq()).is_some_and(|o| o.uuid() != i.uuid())) || oldest.is_some_and(|o| next.first().is_none_or(|n| n.seq() != o));
                let fresh: Vec<ChatItem> = next.iter().filter(|i| c.items.get(&i.seq()).is_none()).cloned().collect();
                ch.items(c.items.set(next, seq));
                let more = c.items.forget_oldest(ITEMS) || (!keep && *has_more) || (keep && c.has_more.v == Some(true));
                if c.has_more.set(more, seq) {
                    ch.state.has_more = Some(more);
                }
                self.chat_info(info, seq, &mut ch);
                let Body::Chat(c) = &mut self.body else { return Out::Nothing };
                if c.progress.v.is_some_and(|p| p.total > 0) && c.progress.set(Progress { done: 0, total: 0 }, seq) {
                    ch.state.progress = Some(Progress { done: 0, total: 0 });
                }
                let all: Vec<ChatItem> = c.items.values().cloned().collect();
                let live = without_screen_read(without_recorded(c.live.values().cloned().collect(), &all), &fresh);
                ch.live(c.live.set(live, seq));
                c.loaded.set(true, seq);
                // the first page, or one that doesn't follow on from what the apps hold: they start over
                if !loaded || rewritten || !ready {
                    return self.rebuild();
                }
            }
            ServerMsg::ChatAppend { items, .. } => {
                let last = c.items.values().next_back().map_or(-1, ChatItem::seq);
                let fresh: Vec<ChatItem> = items.iter().filter(|i| i.seq() > last).cloned().collect();
                if fresh.is_empty() {
                    return Out::Nothing;
                }
                // what the record now holds takes the place of the live text in the same delta
                let live = without_screen_read(without_recorded(c.live.values().cloned().collect(), &fresh), &fresh);
                ch.live(c.live.set(live, seq));
                ch.items(c.items.push(fresh, seq));
                // a snapshot starts at the oldest message kept: earlier ones are there to page back to
                if c.items.forget_oldest(ITEMS) && c.has_more.set(true, seq) {
                    ch.state.has_more = Some(true);
                }
            }
            ServerMsg::ChatInfo { info, .. } => {
                let had = c.loaded();
                self.chat_info(info, seq, &mut ch);
                let Body::Chat(c) = &mut self.body else { return Out::Nothing };
                // as the app does: a chat with no record to read is loaded as it is
                if !had && info.transcript_path.as_deref().is_none_or(str::is_empty) {
                    c.loaded.set(true, seq);
                    return self.rebuild();
                }
            }
            ServerMsg::ChatScreen { screen, .. } => {
                if c.screen.set(screen.clone(), seq) {
                    ch.state.screen = Some(screen.clone());
                }
            }
            ServerMsg::ChatLive { items, .. } => {
                let all: Vec<ChatItem> = c.items.values().cloned().collect();
                ch.live(c.live.set(without_recorded(items.clone(), &all), seq));
            }
            ServerMsg::ChatProgress { done, total, .. } => {
                let p = Progress { done: *done, total: *total };
                if c.progress.set(p, seq) {
                    ch.state.progress = Some(p);
                }
            }
            ServerMsg::ChatDraft { text, source, at, .. } => {
                let d = DraftState { text: text.clone(), source: *source, at: *at };
                if c.draft.set(d.clone(), seq) {
                    ch.state.draft = Some(d);
                }
            }
            _ => return Out::Nothing,
        }
        self.commit(ch)
    }

    fn chat_info(&mut self, info: &ChatInfo, seq: u64, ch: &mut Change) {
        let Body::Chat(c) = &mut self.body else { return };
        let (info, queue) = split_queue(info);
        let info = Box::new(info);
        if c.info.set(info.clone(), seq) {
            ch.state.info = Some(info);
        }
        ch.queue(c.queue.set(queue, seq));
    }

    /// The whole stream as it is now.
    fn snapshot(&self) -> Snapshot {
        let (rows, state) = match &self.body {
            Body::Overview(hosts) => (Rows { hosts: Some(hosts.values().cloned().collect()), ..Rows::default() }, None),
            Body::Chat(c) => (
                Rows {
                    items: Some(c.items.values().cloned().collect()),
                    live: Some(c.live.values().cloned().collect()),
                    queue: Some(c.queue.values().cloned().collect()),
                    ..Rows::default()
                },
                Some(c.state_since(0)),
            ),
            Body::Organize(o) => (o.rows(), None),
            Body::Pages(pages) => (Rows { pages: Some(pages.values().cloned().collect()), ..Rows::default() }, None),
        };
        Snapshot { stream: self.name.clone(), epoch: self.epoch, seq: self.seq, rows, state }
    }

    /// What brings an app at `at` up to now: nothing when it's there, one delta within the epoch,
    /// else a snapshot (None while the stream has nothing to show yet).
    fn catch_up(&self, at: Option<StreamPos>) -> Option<ServerMsg2> {
        if !self.ready {
            return None;
        }
        let Some(at) = at.filter(|a| a.epoch == self.epoch && a.seq <= self.seq) else { return Some(ServerMsg2::Snapshot(self.snapshot())) };
        let mut ch = Change::default();
        let ok = match &self.body {
            Body::Overview(hosts) => hosts.since(at.seq).map(|o| ch.hosts(o)).is_some(),
            Body::Chat(c) => match (c.items.since(at.seq), c.live.since(at.seq), c.queue.since(at.seq)) {
                (Some(i), Some(l), Some(q)) => {
                    ch.items(i);
                    ch.live(l);
                    ch.queue(q);
                    ch.state = c.state_since(at.seq);
                    true
                }
                _ => false,
            },
            Body::Organize(o) => o.since(at.seq).map(|ops| ch.organize(ops)).is_some(),
            Body::Pages(pages) => pages.since(at.seq).map(|o| ch.pages(o)).is_some(),
        };
        if !ok {
            return Some(ServerMsg2::Snapshot(self.snapshot()));
        }
        Some(ServerMsg2::Delta(ch.into_delta(&self.name, self.epoch, at.seq, self.seq)))
    }
}

// ---------- live text and the record ----------

/// The ids the record's items answer to: a live item with one of them is finished.
fn recorded_ids(items: &[ChatItem]) -> HashSet<&str> {
    let mut ids = HashSet::new();
    for i in items {
        ids.insert(i.uuid());
        match i {
            ChatItem::Assistant { msg_id, block, .. } => {
                ids.insert(msg_id.as_str());
                if let AssistantBlock::ToolUse { id, .. } = block {
                    ids.insert(id.as_str());
                }
            }
            ChatItem::ToolResult { tool_use_id, .. } => {
                ids.insert(tool_use_id.as_str());
            }
            _ => {}
        }
    }
    ids
}

/// Live text whose record has come goes: the record outranks it (the rule chat-core applies too).
fn without_recorded(live: Vec<LiveItem>, items: &[ChatItem]) -> Vec<LiveItem> {
    if live.is_empty() || items.is_empty() {
        return live;
    }
    let ids = recorded_ids(items);
    live.into_iter().filter(|l| !ids.contains(l.id.as_str())).collect()
}

/// Text read off the screen ("screen:…") has no record with its id: a new reply on record takes its
/// place, and the watcher sends whatever is still unrecorded a moment later.
fn without_screen_read(live: Vec<LiveItem>, fresh: &[ChatItem]) -> Vec<LiveItem> {
    if !fresh.iter().any(|i| matches!(i, ChatItem::Assistant { block: AssistantBlock::Text { .. }, .. })) {
        return live;
    }
    live.into_iter().filter(|l| !l.id.starts_with("screen:")).collect()
}

// ---------- apps ----------

/// What goes out on a `/ws2` socket: JSON as text, or compressed as binary.
#[derive(Debug, Clone, PartialEq)]
pub enum Wire {
    Text(String),
    Binary(Vec<u8>),
}

/// A frame encoded once for every app it goes to.
struct Encoded {
    json: String,
    zipped: OnceCell<Vec<u8>>,
}

impl Encoded {
    fn new(msg: &ServerMsg2) -> Self {
        let json = serde_json::to_value(msg).map(|v| inyon_core::js::stringify(&v)).unwrap_or_default();
        Encoded { json, zipped: OnceCell::new() }
    }
}

/// Raw DEFLATE (RFC 1951), what `DecompressionStream('deflate-raw')` reads. Level 6 is zlib's own
/// default: on a chat's JSON, higher levels save under 1% more for twice the time.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 6)
}

/// A `/ws2` app's side of the streams.
pub struct Conn {
    /// the streams it follows, and where it is in each (None: it waits for a snapshot)
    at: RefCell<IndexMap<String, Option<StreamPos>>>,
    zip: Cell<bool>,
    out: Box<dyn Fn(Wire)>,
}

impl Conn {
    pub(crate) fn new(out: impl Fn(Wire) + 'static) -> Self {
        Conn { at: RefCell::new(IndexMap::new()), zip: Cell::new(false), out: Box::new(out) }
    }

    pub(crate) fn follows(&self, stream: &str) -> bool {
        self.at.borrow().contains_key(stream)
    }

    /// Sends JSON text, compressed when the app reads that and it's long enough to be worth it.
    pub(crate) fn send_json(&self, json: String) {
        if self.zip.get() && json.len() >= ZIP_MIN {
            (self.out)(Wire::Binary(deflate(json.as_bytes())))
        } else {
            (self.out)(Wire::Text(json))
        }
    }

    fn send(&self, e: &Encoded) {
        if self.zip.get() && e.json.len() >= ZIP_MIN {
            (self.out)(Wire::Binary(e.zipped.get_or_init(|| deflate(e.json.as_bytes())).clone()))
        } else {
            (self.out)(Wire::Text(e.json.clone()))
        }
    }

    fn send_msg(&self, msg: &ServerMsg2) {
        self.send(&Encoded::new(msg));
    }
}

fn conn(c: &ClientRef) -> Option<&Rc<Conn>> {
    c.v2.as_ref()
}

fn followers(stream: &str) -> Vec<ClientRef> {
    all().into_iter().filter(|c| conn(c).is_some_and(|v| v.follows(stream))).collect()
}

/// Brings one app up to date on a stream, by `frame` when it is the very next step for it.
fn sync(c: &ClientRef, stream: &str, frame: Option<(&Delta, &Encoded)>) {
    let Some(v) = conn(c) else { return };
    let Some(at) = v.at.borrow().get(stream).copied() else { return };
    if let Some((d, e)) = frame {
        if at == Some(StreamPos { epoch: d.epoch, seq: d.from }) {
            v.send(e);
            v.at.borrow_mut().insert(stream.to_owned(), Some(StreamPos { epoch: d.epoch, seq: d.seq }));
            return;
        }
    }
    let (msg, now) = STREAMS.with(|s| {
        let s = s.borrow();
        let Some(st) = s.get(stream) else { return (None, None) };
        if at == Some(st.pos()) {
            return (None, None);
        }
        (st.catch_up(at), Some(st.pos()))
    });
    if let Some(msg) = msg {
        v.send_msg(&msg);
        v.at.borrow_mut().insert(stream.to_owned(), now);
    }
}

/// Sends what a change to a stream comes to, to every app following it that isn't quiet. `except`
/// gets the delta without its draft: the app that typed it has it already.
fn publish(stream: &str, out: Out, except: Option<&ClientRef>) {
    let frame = match out {
        Out::Nothing => return,
        Out::Delta(d) => Some(*d),
        Out::Rebuilt => None,
    };
    let encoded = frame.as_ref().map(|d| Encoded::new(&ServerMsg2::Delta(d.clone())));
    let key = (stream == OVERVIEW).then(|| quiet_key(&registry().overview()));
    for c in followers(stream) {
        let mine = except.is_some_and(|x| Rc::ptr_eq(x, &c));
        if c.is_quiet() {
            // the 0.23.1 rule: in the background, the overview only when a machine or an agent's
            // state changed, and then all of it that changed since
            match &key {
                Some(k) if *c.sent_key().borrow() != *k => *c.sent_key().borrow_mut() = k.clone(),
                _ => continue,
            }
            sync(&c, stream, None);
            continue;
        }
        if let Some(k) = &key {
            *c.sent_key().borrow_mut() = k.clone();
        }
        match (&frame, &encoded) {
            (Some(d), Some(_)) if mine => {
                let d = without_draft(d.clone());
                sync(&c, stream, Some((&d, &Encoded::new(&ServerMsg2::Delta(d.clone())))));
            }
            (Some(d), Some(e)) => sync(&c, stream, Some((d, e))),
            _ => sync(&c, stream, None),
        }
    }
}

fn without_draft(mut d: Delta) -> Delta {
    if let Some(s) = &mut d.state {
        s.draft = None;
    }
    if d.state.as_ref().is_some_and(ChatState::is_empty) {
        d.state = None;
    }
    d
}

fn with_stream<T>(name: &str, f: impl FnOnce(&mut Stream) -> T) -> Option<T> {
    STREAMS.with(|s| s.borrow_mut().get_mut(name).map(f))
}

/// The stream, made if it's new; streams nobody followed for a while go first.
fn ensure(name: &str) {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        if s.contains_key(name) {
            return;
        }
        let now = now_ms();
        s.retain(|_, st| st.left_at.is_none_or(|at| now.saturating_sub(at) < KEEP_MS));
        while s.len() >= MAX_STREAMS {
            let Some(oldest) = s.iter().filter_map(|(k, st)| st.left_at.map(|at| (at, k.clone()))).min().map(|(_, k)| k) else { break };
            s.remove(&oldest);
        }
        s.insert(name.to_owned(), Stream::new(name));
    });
}

/// The registry's overview changed. Read only while a `/ws2` app follows it; one that starts
/// following later gets it read then.
pub(crate) fn overview_changed() {
    if followers(OVERVIEW).is_empty() {
        return;
    }
    refresh_overview();
}

fn refresh_overview() {
    ensure(OVERVIEW);
    let hosts = registry().overview();
    if let Some(out) = with_stream(OVERVIEW, |st| st.on_hosts(hosts)) {
        publish(OVERVIEW, out, None);
    }
}

/// How the sessions are organised changed. Read only while a `/ws2` app follows it.
pub(crate) fn organize_changed() {
    if followers(ORGANIZE).is_empty() {
        return;
    }
    refresh_organize();
}

fn refresh_organize() {
    ensure(ORGANIZE);
    let org = inyon_core::features::organize::current();
    if let Some(out) = with_stream(ORGANIZE, |st| st.on_organize(org)) {
        publish(ORGANIZE, out, None);
    }
}

thread_local! {
    /// what the hub does to read the pages again (it answers with [`pages_changed`])
    static PAGES_WANTED: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Sets how the hub reads its pages again when an app starts following them, or comes back from
/// the background. It answers, now or later, with [`pages_changed`].
pub fn on_pages_wanted(f: impl Fn() + 'static) {
    PAGES_WANTED.with(|w| *w.borrow_mut() = Some(Rc::new(f)));
}

fn want_pages() {
    if let Some(f) = PAGES_WANTED.with(|w| w.borrow().clone()) {
        f();
    }
}

/// Whether any `/ws2` app follows the pages, so the hub knows to keep reading them.
pub fn pages_followed() -> bool {
    !followers(PAGES).is_empty()
}

/// Every machine's pages, as `GET /api/pages` lists them, read again by the hub. Taken only while
/// an app follows them.
pub fn pages_changed(pages: Vec<PageEntry>) {
    if followers(PAGES).is_empty() {
        return;
    }
    ensure(PAGES);
    if let Some(out) = with_stream(PAGES, |st| st.on_pages(pages)) {
        publish(PAGES, out, None);
    }
}

/// Feeds a chat's watcher messages into its stream.
struct Feed {
    key: String,
}

impl Subscriber for Feed {
    fn send(&self, msg: &ServerMsg) {
        chat_message(&self.key, msg, None);
    }
}

fn chat_message(key: &str, msg: &ServerMsg, except: Option<&ClientRef>) {
    if let Some(out) = with_stream(key, |st| st.on_chat(msg)) {
        publish(key, out, except);
    }
}

/// `more`'s answer goes to the app that asked, outside the stream.
struct MoreSink {
    to: std::rc::Weak<crate::clients::Client>,
    stream: String,
}

impl Subscriber for MoreSink {
    fn send(&self, msg: &ServerMsg) {
        let (Some(c), ServerMsg::ChatMore { items, has_more, .. }) = (self.to.upgrade(), msg) else { return };
        if let Some(v) = conn(&c) {
            v.send_msg(&ServerMsg2::More { stream: self.stream.clone(), items: items.clone(), has_more: *has_more });
        }
    }
}

fn split_key(key: &str) -> Option<(&str, &str)> {
    let (host, pane) = key.rsplit_once(':')?;
    inyon_core::re!(r"^%\d+(\.\d+)?$").is_match(pane).then_some((host, pane))
}

/// An app follows a stream from `at` (or from nothing).
fn follow(c: &ClientRef, stream: &str, at: Option<StreamPos>) {
    let Some(v) = conn(c) else { return };
    if is_chat(stream) {
        let Some((host, _)) = split_key(stream) else { return };
        let chats = v.at.borrow().keys().filter(|k| is_chat(k)).count();
        if registry().get(host).is_none() || (!v.follows(stream) && chats >= MAX_CHATS) {
            return;
        }
    }
    ensure(stream);
    with_stream(stream, |st| st.left_at = None);
    if stream == OVERVIEW {
        // read now: nothing reads it while no /ws2 app follows it
        refresh_overview();
        *c.sent_key().borrow_mut() = quiet_key(&registry().overview());
    }
    if stream == ORGANIZE {
        refresh_organize();
    }
    v.at.borrow_mut().insert(stream.to_owned(), at);
    if stream == PAGES {
        // read by the hub, which may answer later; the snapshot goes out once it has
        want_pages();
    }
    if stream == OVERVIEW {
        crate::clients::watched_changed();
    }
    let current = with_stream(stream, |st| st.pos());
    if at.is_some() && at == current {
        // up to date: an empty delta says so
        let Some(now) = current else { return };
        v.send_msg(&ServerMsg2::Delta(Delta { stream: stream.to_owned(), epoch: now.epoch, from: now.seq, seq: now.seq, remove: None, replace: None, append: None, state: None }));
    } else {
        sync(c, stream, None);
    }
    if is_chat(stream) {
        watch(stream);
    }
}

/// A chat's stream, rather than the overview, organize or pages.
fn is_chat(stream: &str) -> bool {
    stream != OVERVIEW && stream != ORGANIZE && stream != PAGES
}

/// Subscribes a chat stream to its watcher, once the machine reports the pane.
fn watch(key: &str) {
    if with_stream(key, |st| st.feed.is_some()).unwrap_or(true) {
        return;
    }
    let Some((host, pane)) = split_key(key) else { return };
    let feed: Sub = Rc::new(Feed { key: key.to_owned() });
    with_stream(key, |st| st.feed = Some(feed.clone()));
    let (k, h, p) = (key.to_owned(), host.to_owned(), pane.to_owned());
    when_pane(host, pane, move || {
        // still followed, by the same feed
        let same = with_stream(&k, |st| st.feed.as_ref().is_some_and(|f| Rc::ptr_eq(f, &feed))).unwrap_or(false);
        if same {
            watchers().get(&h, &p).subscribe(feed);
        }
    });
}

/// An app stops following a stream. A chat nobody follows lets its watcher go (it stops after two
/// minutes, as for v1) but keeps its rows a while, for an app that comes back.
fn leave(c: &ClientRef, stream: &str) {
    let Some(v) = conn(c) else { return };
    if v.at.borrow_mut().shift_remove(stream).is_none() {
        return;
    }
    if stream == OVERVIEW {
        crate::clients::watched_changed();
    }
    if !followers(stream).is_empty() {
        return;
    }
    let feed = with_stream(stream, |st| {
        st.left_at = Some(now_ms());
        st.feed.take()
    })
    .flatten();
    if let (Some(feed), Some((host, pane))) = (feed, split_key(stream)) {
        if let Some(w) = watchers().peek(host, pane) {
            w.unsubscribe(&feed);
        }
    }
}

/// The app went away.
pub(crate) fn close(c: &ClientRef) {
    let Some(v) = conn(c) else { return };
    let streams: Vec<String> = v.at.borrow().keys().cloned().collect();
    for s in streams {
        leave(c, &s);
    }
}

/// A quiet app came back: every stream it follows, up to date.
pub(crate) fn caught_up(c: &ClientRef) {
    let Some(v) = conn(c) else { return };
    let streams: Vec<String> = v.at.borrow().keys().cloned().collect();
    for s in streams {
        if s == OVERVIEW {
            refresh_overview();
            *c.sent_key().borrow_mut() = quiet_key(&registry().overview());
        }
        if s == ORGANIZE {
            refresh_organize();
        }
        if s == PAGES {
            want_pages();
        }
        sync(c, &s, None);
    }
}

fn pos_of(v: Option<&Value>) -> Option<StreamPos> {
    serde_json::from_value(v?.clone()).ok()
}

/// One message from a `/ws2` app (raw JSON text).
pub fn client_message2(c: &ClientRef, text: &str) {
    let Some(v) = conn(c) else { return };
    let Some(m) = inyon_core::js::json::parse(text) else { return };
    let Some(t) = m.get("t").and_then(Value::as_str) else { return };
    match t {
        "hello" => {
            let wants = m.get("zip").and_then(Value::as_array).is_some_and(|z| z.iter().any(|z| z.as_str() == Some("deflate-raw")));
            // the answer goes out before compression starts, so any app can read it
            v.send_msg(&ServerMsg2::Hello { v: PROTOCOL_V2, zip: wants.then_some(Zip::DeflateRaw) });
            v.zip.set(wants);
        }
        "sub" => {
            let Some(stream) = s(&m, "stream") else { return };
            follow(c, stream, pos_of(m.get("at")));
        }
        "unsub" => {
            if let Some(stream) = s(&m, "stream") {
                leave(c, stream);
            }
        }
        "more" => {
            let Some((host, pane)) = s(&m, "stream").and_then(split_key) else { return };
            if !v.follows(&format!("{host}:{pane}")) {
                return;
            }
            if let Some(w) = watchers().peek(host, pane) {
                let before = crate::http::js_number(m.get("before"));
                let sub: Sub = Rc::new(MoreSink { to: Rc::downgrade(c), stream: format!("{host}:{pane}") });
                spawn(async move { w.more(&sub, if before.is_finite() { before as i64 } else { i64::MAX }).await });
            }
        }
        "draft" => {
            let Some(text) = s(&m, "text") else { return };
            let (host, pane) = (raw(&m, "host"), raw(&m, "pane"));
            let key = format!("{host}:{pane}");
            if text.encode_utf16().count() > 100_000 || registry().get(&host).is_none() || !v.follows(&key) {
                return;
            }
            let Some(w) = watchers().peek(&host, &pane) else { return };
            let feed = with_stream(&key, |st| st.feed.clone()).flatten();
            // the watcher shares it with v1 apps and the terminal; this stream hears it here, so
            // the app that typed it isn't sent its own text back
            w.draft_from_app(text, feed.as_ref());
            let d = inyon_core::claude::drafts::get_draft(&key);
            let stored = d.as_ref().map_or("", |d| d.text.as_str());
            if stored == text {
                let at = d.as_ref().map_or(now_ms(), |d| d.at as u64);
                let msg = ServerMsg::ChatDraft { key: key.clone(), text: text.to_owned(), source: d.as_ref().map_or(DraftSource::App, |d| d.source), at };
                chat_message(&key, &msg, Some(c));
            }
        }
        "quiet" => crate::clients::set_quiet(c, m.get("on").and_then(Value::as_bool).unwrap_or(false)),
        // these follow v1's streams, which a /ws2 app doesn't
        "sub.overview" | "sub.chat" | "unsub.chat" | "chat.more" => {}
        _ => crate::clients::client_message(c, text),
    }
}

#[cfg(test)]
pub(crate) fn set_gone_cap(n: usize) {
    GONE_CAP.with(|g| g.set(n));
}

#[cfg(test)]
pub(crate) fn feed_for_test(key: &str, msg: &ServerMsg) {
    chat_message(key, msg, None);
}

#[cfg(test)]
pub(crate) fn forget_all() {
    STREAMS.with(|s| s.borrow_mut().clear());
}

#[cfg(test)]
#[path = "streams_tests.rs"]
mod tests;
