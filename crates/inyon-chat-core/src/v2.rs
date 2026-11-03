//! Protocol v2 (`/ws2`, see inyon-protocol's `v2.md`): the hub's frames applied to the same chat
//! and overview the v1 messages build, so a chat gives the same [`Splice`]s either way.
//!
//! Each stream keeps a [`Cursor`]: the epoch and seq of the last frame applied, and deltas that came
//! early. A frame is applied, dropped as a duplicate, held until the ones before it come, or answered
//! with a resync (`sub` again with the cursor), by the table in `v2.md`. Large frames come
//! compressed as binary messages; [`decode`] reads either kind.

use inyon_protocol::v2::{join_queue, page_key};
use inyon_protocol::{ChatItem, ChatState, ClientMsg2, Delta, HostState, Keys, LiveItem, Organize, PageEntry, QueuedMsg, Rows, ServerMsg2, Snapshot, StreamPos};

use crate::chat_list::{sends, without_recorded, Chat, ChatData, Draft, Splice};
use crate::pending::settle;

/// A frame decompressed is at most this long; anything larger is refused.
const MAX_FRAME: usize = 64 * 1024 * 1024;
/// deltas held while one before them is missing
const EARLY: usize = 64;

/// A `/ws2` message: text as it came, or a binary one decompressed (raw DEFLATE). Something that
/// isn't a v2 frame, such as a v1 `notify`, comes back as `Err` with its text, for the v1 reader.
pub fn decode(text: Option<&str>, binary: Option<&[u8]>) -> Result<ServerMsg2, Option<String>> {
    let text = match (text, binary) {
        (Some(t), _) => t.to_owned(),
        (None, Some(b)) => {
            let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(b, MAX_FRAME).map_err(|_| None)?;
            String::from_utf8(raw).map_err(|_| None)?
        }
        (None, None) => return Err(None),
    };
    serde_json::from_str::<ServerMsg2>(&text).map_err(|_| Some(text))
}

/// What to do with a frame, given where the stream is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Apply,
    /// seen already
    Drop,
    /// one before it is missing: keep it for when that comes
    Hold,
    /// it can't be applied: ask the hub to bring the stream up to date
    Resync,
}

/// The table in `v2.md`.
pub fn verdict(at: Option<StreamPos>, msg: &ServerMsg2) -> Verdict {
    match msg {
        ServerMsg2::Snapshot(s) => match at {
            Some(a) if a.epoch == s.epoch && s.seq <= a.seq => Verdict::Drop,
            _ => Verdict::Apply,
        },
        ServerMsg2::Delta(d) => match at {
            Some(a) if a.epoch != d.epoch => Verdict::Resync,
            Some(a) if d.seq <= a.seq => Verdict::Drop,
            Some(a) if d.from == a.seq => Verdict::Apply,
            Some(a) if d.from > a.seq => Verdict::Hold,
            _ => Verdict::Resync,
        },
        _ => Verdict::Drop,
    }
}

/// Where one stream is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cursor {
    at: Option<StreamPos>,
    early: Vec<Delta>,
    /// a resync was asked for and nothing has applied since
    asked: bool,
}

impl Cursor {
    /// The last frame applied: what a reconnect sends in `sub`.
    pub fn at(&self) -> Option<StreamPos> {
        self.at
    }

    /// Takes a frame: `apply` runs for it and for any held deltas that follow on from it, in order.
    /// Returns whether to resync (asked once until something applies again).
    pub fn take(&mut self, msg: &ServerMsg2, mut apply: impl FnMut(&ServerMsg2)) -> bool {
        match verdict(self.at, msg) {
            Verdict::Apply => {
                apply(msg);
                self.asked = false;
                self.at = match msg {
                    ServerMsg2::Snapshot(s) => Some(StreamPos { epoch: s.epoch, seq: s.seq }),
                    ServerMsg2::Delta(d) => Some(StreamPos { epoch: d.epoch, seq: d.seq }),
                    _ => self.at,
                };
                self.drain(&mut apply);
                false
            }
            Verdict::Drop => false,
            Verdict::Hold => {
                if let ServerMsg2::Delta(d) = msg {
                    if self.early.len() < EARLY && !self.early.iter().any(|e| e.seq == d.seq && e.epoch == d.epoch) {
                        self.early.push(d.clone());
                    }
                }
                !std::mem::replace(&mut self.asked, true)
            }
            Verdict::Resync => !std::mem::replace(&mut self.asked, true),
        }
    }

    fn drain(&mut self, apply: &mut impl FnMut(&ServerMsg2)) {
        loop {
            let Some(at) = self.at else { return };
            self.early.retain(|d| d.epoch == at.epoch && d.seq > at.seq);
            let Some(i) = self.early.iter().position(|d| d.from == at.seq) else { return };
            let d = self.early.remove(i);
            self.at = Some(StreamPos { epoch: d.epoch, seq: d.seq });
            apply(&ServerMsg2::Delta(d));
        }
    }
}

/// What a frame did to a chat: how its rows changed, and a message for the hub when the stream needs
/// bringing up to date.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Applied {
    pub splices: Vec<Splice>,
    pub resync: Option<ClientMsg2>,
}

/// A chat's place in its stream and the stream's send queue (the hub's rows, put back into the
/// chat's info as v1 has them so the rows come out the same).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatStream {
    pub cursor: Cursor,
    pub queue: Vec<QueuedMsg>,
}

/// Removes, replaces and appends rows of one collection by key.
fn patch_rows<V: Clone, K: PartialEq>(list: &mut Vec<V>, key: impl Fn(&V) -> K, remove: Option<&[K]>, replace: Option<&[V]>, append: Option<&[V]>) -> bool {
    let mut changed = false;
    if let Some(keys) = remove {
        let n = list.len();
        list.retain(|v| !keys.contains(&key(v)));
        changed |= list.len() != n;
    }
    for v in replace.unwrap_or_default() {
        if let Some(i) = list.iter().rposition(|x| key(x) == key(v)) {
            list[i] = v.clone();
            changed = true;
        }
    }
    for v in append.unwrap_or_default() {
        match list.iter().rposition(|x| key(x) == key(v)) {
            Some(i) => list[i] = v.clone(),
            None => list.push(v.clone()),
        }
        changed = true;
    }
    changed
}

impl ChatData {
    fn set_state(&mut self, s: &ChatState, queue: &[QueuedMsg], queue_changed: bool) {
        if let Some(info) = &s.info {
            let mut info = (**info).clone();
            join_queue(&mut info, queue);
            self.loaded = self.loaded || info.transcript_path.as_deref().is_none_or(str::is_empty);
            self.info = Some(info);
        } else if queue_changed {
            if let Some(info) = &mut self.info {
                join_queue(info, queue);
            }
        }
        if let Some(screen) = &s.screen {
            self.screen = Some((**screen).clone());
        }
        if let Some(d) = &s.draft {
            self.draft = Some(Draft { text: d.text.clone(), source: d.source, at: d.at });
        }
        if let Some(p) = s.progress {
            self.catching_up = (p.total > 0).then(|| (p.done as f64 / p.total as f64).min(1.0));
        }
        if let Some(more) = s.has_more {
            self.has_more = more;
        }
        if s.loaded == Some(true) {
            self.loaded = true;
        }
    }

    /// A chat snapshot. Until the hub has read the agent's record (`loaded: false`) it has no
    /// items, and the chat keeps showing what it had, as v1's info before the first page does.
    fn take_snapshot(&mut self, s: &Snapshot, queue: &mut Vec<QueuedMsg>) -> bool {
        *queue = s.rows.queue.clone().unwrap_or_default();
        let state = s.state.clone().unwrap_or_default();
        let loaded = state.loaded == Some(true);
        if loaded {
            self.items = s.rows.items.clone().unwrap_or_default();
            self.has_more = state.has_more.unwrap_or(false);
            self.stale = false;
            self.catching_up = None;
        }
        self.live = without_recorded(Some(s.rows.live.clone().unwrap_or_default()), &self.items);
        self.set_state(&state, queue, true);
        if loaded {
            self.pending = settle(&self.pending, &self.items, sends(self.info.as_ref()));
        }
        loaded
    }

    fn take_delta(&mut self, d: &Delta, queue: &mut Vec<QueuedMsg>) -> bool {
        let none = Keys::default();
        let empty = Rows::default();
        let (rm, rp, ap) = (d.remove.as_ref().unwrap_or(&none), d.replace.as_ref().unwrap_or(&empty), d.append.as_ref().unwrap_or(&empty));
        let items = patch_rows(&mut self.items, ChatItem::seq, rm.items.as_deref(), rp.items.as_deref(), ap.items.as_deref());
        let mut live = self.live.take().unwrap_or_default();
        patch_rows(&mut live, |l: &LiveItem| l.id.clone(), rm.live.as_deref(), rp.live.as_deref(), ap.live.as_deref());
        self.live = without_recorded(Some(live), &self.items);
        let queued = patch_rows(queue, QueuedMsg::key, rm.queue.as_deref(), rp.queue.as_deref(), ap.queue.as_deref());
        self.set_state(d.state.as_ref().unwrap_or(&ChatState::default()), queue, queued);
        if items {
            let fresh: Vec<ChatItem> = rp.items.iter().chain(ap.items.iter()).flatten().cloned().collect();
            self.pending = settle(&self.pending, &fresh, sends(self.info.as_ref()));
        }
        items
    }
}

impl Chat {
    /// The last frame of its stream applied: what a reconnect sends in `sub`.
    pub fn position(&self) -> Option<StreamPos> {
        self.stream.cursor.at()
    }

    /// Applies a `/ws2` frame about this chat. `more` (earlier messages) goes in like v1's
    /// `chat.more`; everything else by its epoch and seq.
    pub fn apply2(&mut self, msg: &ServerMsg2, now: u64) -> Applied {
        if let ServerMsg2::More { stream, items, has_more } = msg {
            let v1 = inyon_protocol::ServerMsg::ChatMore { key: stream.clone(), items: items.clone(), has_more: *has_more };
            return Applied { splices: self.apply(&v1, now), resync: None };
        }
        let mut changed = false;
        let (data, stream) = (&mut self.data, &mut self.stream);
        let mut cursor = std::mem::take(&mut stream.cursor);
        let resync = cursor.take(msg, |m| match m {
            ServerMsg2::Snapshot(s) => changed |= data.take_snapshot(s, &mut stream.queue),
            ServerMsg2::Delta(d) => changed |= data.take_delta(d, &mut stream.queue),
            _ => {}
        });
        self.stream.cursor = cursor;
        let resync = resync.then(|| ClientMsg2::Sub { stream: msg.stream().unwrap_or_default().to_owned(), at: self.position() });
        Applied { splices: self.settled(changed, now), resync }
    }
}

/// The overview stream: the machines as the hub has them, to hand to [`crate::Store::apply`] as a
/// v1 `overview` whenever they change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overview2 {
    pub cursor: Cursor,
    pub hosts: Vec<HostState>,
}

impl Overview2 {
    /// Applies an overview frame. Returns the machines when they changed, and a message for the hub
    /// when the stream needs bringing up to date.
    pub fn apply(&mut self, msg: &ServerMsg2) -> (Option<Vec<HostState>>, Option<ClientMsg2>) {
        let mut changed = false;
        let hosts = &mut self.hosts;
        let resync = self.cursor.take(msg, |m| match m {
            ServerMsg2::Snapshot(s) => {
                *hosts = s.rows.hosts.clone().unwrap_or_default();
                changed = true;
            }
            ServerMsg2::Delta(d) => {
                let none = Keys::default();
                let empty = Rows::default();
                let (rm, rp, ap) = (d.remove.as_ref().unwrap_or(&none), d.replace.as_ref().unwrap_or(&empty), d.append.as_ref().unwrap_or(&empty));
                changed |= patch_rows(hosts, |h: &HostState| h.config.id.clone(), rm.hosts.as_deref(), rp.hosts.as_deref(), ap.hosts.as_deref());
            }
            _ => {}
        });
        let resync = resync.then(|| ClientMsg2::Sub { stream: inyon_protocol::v2::OVERVIEW.to_owned(), at: self.cursor.at() });
        (changed.then(|| self.hosts.clone()), resync)
    }
}

/// The organize stream (from 0.24): how the hub organises sessions, the same [`Organize`] a v1
/// overview carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Organize2 {
    pub cursor: Cursor,
    pub org: Organize,
}

impl Organize2 {
    /// Applies an organize frame. Returns all of it when it changed, and a message for the hub when
    /// the stream needs bringing up to date.
    pub fn apply(&mut self, msg: &ServerMsg2) -> (Option<Organize>, Option<ClientMsg2>) {
        let mut changed = false;
        let org = &mut self.org;
        let resync = self.cursor.take(msg, |m| match m {
            ServerMsg2::Snapshot(s) => {
                let r = &s.rows;
                *org = Organize {
                    v: inyon_protocol::ORGANIZE_VERSION,
                    sessions: r.sessions.clone().unwrap_or_default(),
                    projects: r.projects.clone().unwrap_or_default(),
                    groups: r.groups.clone().unwrap_or_default(),
                    spaces: r.spaces.clone().unwrap_or_default(),
                    sends: r.sends.clone().filter(|l| !l.is_empty()),
                    prompts: r.prompts.clone().filter(|l| !l.is_empty()),
                    sleep: r.sleep.clone().filter(|l| !l.is_empty()),
                };
                changed = true;
            }
            ServerMsg2::Delta(d) => {
                let none = Keys::default();
                let empty = Rows::default();
                let (rm, rp, ap) = (d.remove.as_ref().unwrap_or(&none), d.replace.as_ref().unwrap_or(&empty), d.append.as_ref().unwrap_or(&empty));
                changed |= patch_rows(&mut org.sessions, |m| m.id.clone(), rm.sessions.as_deref(), rp.sessions.as_deref(), ap.sessions.as_deref());
                changed |= patch_rows(&mut org.projects, |p| p.id.clone(), rm.projects.as_deref(), rp.projects.as_deref(), ap.projects.as_deref());
                changed |= patch_rows(&mut org.groups, |g| g.id.clone(), rm.groups.as_deref(), rp.groups.as_deref(), ap.groups.as_deref());
                changed |= patch_rows(&mut org.spaces, |s| s.id.clone(), rm.spaces.as_deref(), rp.spaces.as_deref(), ap.spaces.as_deref());
                changed |= patch_opt(&mut org.sends, |s| s.id.clone(), rm.sends.as_deref(), rp.sends.as_deref(), ap.sends.as_deref());
                changed |= patch_opt(&mut org.prompts, |q| q.id.clone(), rm.prompts.as_deref(), rp.prompts.as_deref(), ap.prompts.as_deref());
                changed |= patch_opt(&mut org.sleep, |r| r.scope.clone(), rm.sleep.as_deref(), rp.sleep.as_deref(), ap.sleep.as_deref());
            }
            _ => {}
        });
        let resync = resync.then(|| ClientMsg2::Sub { stream: inyon_protocol::v2::ORGANIZE.to_owned(), at: self.cursor.at() });
        (changed.then(|| self.org.clone()), resync)
    }
}

/// The pages stream (from 0.25): every machine's pages with their progress, as `GET /api/pages`
/// lists them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pages2 {
    pub cursor: Cursor,
    pub pages: Vec<PageEntry>,
}

impl Pages2 {
    /// Applies a pages frame. Returns the pages when they changed, and a message for the hub when
    /// the stream needs bringing up to date.
    pub fn apply(&mut self, msg: &ServerMsg2) -> (Option<Vec<PageEntry>>, Option<ClientMsg2>) {
        let mut changed = false;
        let pages = &mut self.pages;
        let resync = self.cursor.take(msg, |m| match m {
            ServerMsg2::Snapshot(s) => {
                *pages = s.rows.pages.clone().unwrap_or_default();
                changed = true;
            }
            ServerMsg2::Delta(d) => {
                let none = Keys::default();
                let empty = Rows::default();
                let (rm, rp, ap) = (d.remove.as_ref().unwrap_or(&none), d.replace.as_ref().unwrap_or(&empty), d.append.as_ref().unwrap_or(&empty));
                changed |= patch_rows(pages, page_key, rm.pages.as_deref(), rp.pages.as_deref(), ap.pages.as_deref());
            }
            _ => {}
        });
        let resync = resync.then(|| ClientMsg2::Sub { stream: inyon_protocol::v2::PAGES.to_owned(), at: self.cursor.at() });
        (changed.then(|| self.pages.clone()), resync)
    }
}

/// [`patch_rows`] on a list the record leaves out while it's empty.
fn patch_opt<T: Clone>(rows: &mut Option<Vec<T>>, key: impl Fn(&T) -> String, remove: Option<&[String]>, replace: Option<&[T]>, append: Option<&[T]>) -> bool {
    let mut list = rows.take().unwrap_or_default();
    let changed = patch_rows(&mut list, key, remove, replace, append);
    *rows = (!list.is_empty()).then_some(list);
    changed
}

#[cfg(test)]
#[path = "v2_tests.rs"]
mod tests;
