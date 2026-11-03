#![doc = include_str!("v2.md")]

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use ts_rs::TS;

use crate::organize::{DelayedSend, Project, ProjectGroup, SavedPrompt, SessionMark, SleepRule, Space};
use crate::types::{ChatInfo, ChatItem, DraftSource, HeldPrompt, HostState, LiveItem, PageEntry, ScreenState, SendState, SendStatus};

/// The protocol version `hello` names.
pub const PROTOCOL_V2: u32 = 2;

/// The stream of machines and panes. Every other stream is a chat, named by its key (`host:pane`),
/// or the organize or pages stream.
pub const OVERVIEW: &str = "overview";

/// The stream of how sessions are organised (from 0.24): pins, parking, snoozes, tags, notes,
/// projects, messages sent later, sleep rules and saved prompts, as `GET /api/organize` has them.
pub const ORGANIZE: &str = "organize";

/// The stream of Pages (from 0.25): every machine's pages with their progress, as `GET /api/pages`
/// lists them, kept up to date while an app follows it.
pub const PAGES: &str = "pages";

/// A page's key in the pages stream: `<host>/<slug>`.
pub fn page_key(p: &PageEntry) -> String {
    format!("{}/{}", p.host, p.slug)
}

/// A frame at least this long (its JSON, in bytes) goes out compressed to an app that asked.
pub const ZIP_MIN: usize = 1024;

/// How a large frame is compressed: raw DEFLATE (RFC 1951), sent as a binary WebSocket message.
/// A browser reads it with `new DecompressionStream('deflate-raw')`, a Rust app with miniz_oxide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub enum Zip {
    #[serde(rename = "deflate-raw")]
    DeflateRaw,
}

/// Where an app is in one stream: the last frame it applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct StreamPos {
    pub epoch: u64,
    pub seq: u64,
}

/// What the app sends on `/ws2`. The v1 messages that aren't about streams (`draft`, `term.*`,
/// `ping`, `quiet`) are sent as they are; `sub.overview`, `sub.chat`, `unsub.chat` and `chat.more`
/// are ignored there.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "t")]
#[ts(optional_fields)]
pub enum ClientMsg2 {
    /// first, optionally: the version the app speaks and the compression it reads
    #[serde(rename = "hello")]
    Hello { v: u32, zip: Option<Vec<Zip>> },
    /// follow a stream; `at` is where the app left it, to get only what it missed
    #[serde(rename = "sub")]
    Sub { stream: String, at: Option<StreamPos> },
    #[serde(rename = "unsub")]
    Unsub { stream: String },
    /// earlier messages of a chat, before the item with seq `before` (answered with `more`)
    #[serde(rename = "more")]
    More { stream: String, before: i64 },
}

/// One message on its way to the agent, as every device sees it: held by Inyon until the agent is
/// free, sent through Inyon and followed until the agent's record has it, or waiting in the agent's
/// own queue. The hub owns this list; a chat's `info` carries none of it on `/ws2`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct QueuedMsg {
    pub kind: QueueKind,
    /// the held prompt's id, the send's id, or the place in the agent's queue
    pub id: String,
    pub text: String,
    /// unix ms; not known for the agent's queue
    pub at: Option<u64>,
    /// send only: how far it got (`delivered` stays until the hub's ledger lets it go, so each
    /// device can settle its own bubble by id)
    pub state: Option<SendStatus>,
    /// send only, once delivered: the item it became
    pub uuid: Option<String>,
    pub error: Option<String>,
    /// send only: a change made with the app's controls, not shown as a message
    pub setting: Option<bool>,
}

/// held: Inyon types it once the agent is free · send: sent through Inyon · agent: in the agent's
/// own queue, as its screen or record shows it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum QueueKind {
    Held,
    Send,
    Agent,
}

impl QueuedMsg {
    /// Its key in the stream, unique among the queue's rows.
    pub fn key(&self) -> String {
        let kind = match self.kind {
            QueueKind::Held => "held",
            QueueKind::Send => "send",
            QueueKind::Agent => "agent",
        };
        format!("{kind}:{}", self.id)
    }
}

/// Rows, by collection. The overview stream has `hosts` (keyed by `config.id`); a chat has `items`
/// (keyed by `seq`), `live` (keyed by `id`) and `queue` (keyed by [`QueuedMsg::key`]); the organize
/// stream has `sessions`, `projects`, `groups`, `spaces`, `sends` and `prompts` (each keyed by
/// `id`) and `sleep` (keyed by `scope`); the pages stream has `pages` (keyed by [`page_key`]).
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Rows {
    pub hosts: Option<Vec<HostState>>,
    pub items: Option<Vec<ChatItem>>,
    pub live: Option<Vec<LiveItem>>,
    pub queue: Option<Vec<QueuedMsg>>,
    pub sessions: Option<Vec<SessionMark>>,
    pub projects: Option<Vec<Project>>,
    pub groups: Option<Vec<ProjectGroup>>,
    pub spaces: Option<Vec<Space>>,
    pub sends: Option<Vec<DelayedSend>>,
    pub prompts: Option<Vec<SavedPrompt>>,
    pub sleep: Option<Vec<SleepRule>>,
    pub pages: Option<Vec<PageEntry>>,
}

/// Row keys, by collection, for `remove`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Keys {
    pub hosts: Option<Vec<String>>,
    pub items: Option<Vec<i64>>,
    pub live: Option<Vec<String>>,
    pub queue: Option<Vec<String>>,
    pub sessions: Option<Vec<String>>,
    pub projects: Option<Vec<String>>,
    pub groups: Option<Vec<String>>,
    pub spaces: Option<Vec<String>>,
    pub sends: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub sleep: Option<Vec<String>>,
    pub pages: Option<Vec<String>>,
}

/// The unsent message for a pane, shared by every device and the terminal's input box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DraftState {
    pub text: String,
    pub source: DraftSource,
    pub at: u64,
}

/// Reading what was said while nobody watched: bytes read of how many (0 of 0 once it's done).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// A chat's single values. In a snapshot every one the hub has is there; in a delta only those
/// that changed, and a missing one is unchanged.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ChatState {
    /// without `sends`, `held` and `queue`, which are the `queue` rows
    pub info: Option<Box<ChatInfo>>,
    pub screen: Option<Box<ScreenState>>,
    pub draft: Option<DraftState>,
    pub progress: Option<Progress>,
    /// there are messages before the first item (ask with `more`)
    pub has_more: Option<bool>,
    /// the agent's record has been read: until then `items` is empty and an app keeps what it showed
    pub loaded: Option<bool>,
}

impl ChatState {
    pub fn is_empty(&self) -> bool {
        self == &ChatState::default()
    }
}

/// A stream's whole state at `epoch`/`seq`: the app drops what it had for the stream and takes
/// this. Its `epoch` is new whenever the hub rebuilt the stream's history.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Snapshot {
    pub stream: String,
    pub epoch: u64,
    pub seq: u64,
    pub rows: Rows,
    pub state: Option<ChatState>,
}

/// The change from `from` to `seq` in one epoch, applied in this order: `remove` rows by key,
/// `replace` rows in place by key, `append` rows at the end of their collection, then `state`.
/// A live change has `from = seq - 1`; catching up after a gap is one delta from the app's `seq`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Delta {
    pub stream: String,
    pub epoch: u64,
    pub from: u64,
    pub seq: u64,
    pub remove: Option<Keys>,
    pub replace: Option<Rows>,
    pub append: Option<Rows>,
    pub state: Option<ChatState>,
}

impl Delta {
    /// No change at all: what an app that is already up to date gets for its `sub`.
    pub fn is_empty(&self) -> bool {
        self.remove.is_none() && self.replace.is_none() && self.append.is_none() && self.state.is_none()
    }
}

/// What the hub sends on `/ws2`. Alongside these it sends the v1 messages that aren't about streams
/// as they are: `notify`, `page.notify`, `companion.reply`, `term.data`, `term.exit` and `pong`.
// a frame is made and sent (or read and applied) at once, never kept in numbers, so its size
// against `hello`'s doesn't matter
#[allow(clippy::large_enum_variant)]
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t")]
#[ts(optional_fields)]
pub enum ServerMsg2 {
    /// the answer to `hello`: the compression the hub will use, if any
    #[serde(rename = "hello")]
    Hello { v: u32, zip: Option<Zip> },
    #[serde(rename = "snapshot")]
    Snapshot(Snapshot),
    #[serde(rename = "delta")]
    Delta(Delta),
    /// earlier messages of a chat, for `more`; not part of the stream's sequence
    #[serde(rename = "more")]
    More {
        stream: String,
        items: Vec<ChatItem>,
        #[serde(rename = "hasMore")]
        has_more: bool,
    },
}

impl ServerMsg2 {
    /// The stream a frame belongs to.
    pub fn stream(&self) -> Option<&str> {
        match self {
            ServerMsg2::Snapshot(s) => Some(&s.stream),
            ServerMsg2::Delta(d) => Some(&d.stream),
            ServerMsg2::More { stream, .. } => Some(stream),
            ServerMsg2::Hello { .. } => None,
        }
    }
}

/// Splits a chat's info into what `/ws2` sends as `state.info` and the `queue` rows: held prompts,
/// then sends, then the agent's own queue.
pub fn split_queue(info: &ChatInfo) -> (ChatInfo, Vec<QueuedMsg>) {
    let mut rest = info.clone();
    let mut queue = Vec::new();
    for h in rest.held.take().unwrap_or_default() {
        queue.push(QueuedMsg { kind: QueueKind::Held, id: h.id, text: h.text, at: Some(h.at), state: None, uuid: None, error: h.error, setting: None });
    }
    for s in rest.sends.take().unwrap_or_default() {
        queue.push(QueuedMsg { kind: QueueKind::Send, id: s.id, text: s.text, at: Some(s.at), state: Some(s.state), uuid: s.uuid, error: s.error, setting: s.setting });
    }
    for (i, text) in std::mem::take(&mut rest.queue).into_iter().enumerate() {
        queue.push(QueuedMsg { kind: QueueKind::Agent, id: i.to_string(), text, at: None, state: None, uuid: None, error: None, setting: None });
    }
    (rest, queue)
}

/// Puts the `queue` rows back into a chat's info, as v1 sends it (the opposite of [`split_queue`]).
pub fn join_queue(info: &mut ChatInfo, queue: &[QueuedMsg]) {
    let held: Vec<HeldPrompt> =
        queue.iter().filter(|q| q.kind == QueueKind::Held).map(|q| HeldPrompt { id: q.id.clone(), text: q.text.clone(), at: q.at.unwrap_or(0), error: q.error.clone() }).collect();
    let sends: Vec<SendState> = queue
        .iter()
        .filter(|q| q.kind == QueueKind::Send)
        .map(|q| SendState {
            id: q.id.clone(),
            text: q.text.clone(),
            at: q.at.unwrap_or(0),
            state: q.state.unwrap_or(SendStatus::Sent),
            uuid: q.uuid.clone(),
            error: q.error.clone(),
            setting: q.setting,
        })
        .collect();
    info.held = (!held.is_empty()).then_some(held);
    info.sends = (!sends.is_empty()).then_some(sends);
    info.queue = queue.iter().filter(|q| q.kind == QueueKind::Agent).map(|q| q.text.clone()).collect();
}
