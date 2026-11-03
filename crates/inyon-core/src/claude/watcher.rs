//! Port of claude/watcher.ts: one watcher per agent pane someone is looking at. It follows the
//! session's transcript and polls the pane's screen (for prompts, the spinner and the permission
//! mode), pushing changes to subscribers, and keeps the draft in step with the terminal's input box.
//!
//! A watcher's state is in a `RefCell` that is never borrowed across an await or while a
//! subscriber or the registry is called; timers and streams hold a weak reference to it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::{Rc, Weak};

use futures_util::future::{join, LocalBoxFuture};
use indexmap::IndexMap;
use inyon_protocol::{
    chat_key, AgentKind, AgentPaneInfo, ChatInfo, ChatItem, HostKind, HostStatus, LiveItem, LiveKind, PromptOption, RemoteControl, ScreenPrompt, ScreenState, SendStatus,
    ServerMsg, SubagentInfo,
};
use serde_json::Value;

use super::chat_cache::{load_chat, save_chat, CachedChat, ChatAt, KEEP as CACHE_KEEP, READER};
use super::delivery::{has_paste_preview, input_contains, SendLedger};
use super::drafts::{get_draft, mirror_to_terminal, set_draft, squash, Draft, DraftSource};
use super::driving::{is_driving, start_driving};
use super::screen::parse_screen;
use super::screen_reply::{carry_on, reads_reply, screen_reply, unrecorded};
use super::transcript::{load_meta, new_meta, normalize, save_meta, with_seq, TranscriptMeta};
use super::transcript_tail::{codex_meta_command, meta_command, parse_size, range_command, size_command, tail_command, TranscriptReader};
use crate::agents::antigravity::AntigravityDoc;
use crate::agents::codex::{codex_history, normalize_codex};
use crate::agents::codex_live::{codex_live_command, codex_thread_id, CodexLiveState};
use crate::agents::copilot::normalize_copilot;
use crate::agents::gemini::GeminiDoc;
use crate::agents::opencode::{opencode_items, opencode_post, parse_endpoint, via_open_code_api};
use crate::agents::opencode_api::OpenCodeClient;
use crate::agents::opencode_controls::opencode_live;
use crate::agents::opencode_db::{is_ocdb, ocdb_rows_of, ocdb_session, ocdb_session_info_of, ocdb_shape_of, ocdb_stamp_of, OcdbCache, OcdbSessionInfo};
use crate::agents::opencode_questions::{OpenCodeQuestion, OpenCodeQuestionFlow};
use crate::clock::now_ms;
use crate::hosts::host::{shq, Host, StreamHandle};
use crate::hosts::http_error::{HubError, HubResult};
use crate::hosts::registry::{pane_title, registry, RegistryEvent};
use crate::js::json::{self, Js};
use crate::js::{date, stringify};
use crate::platform::console;
use crate::platform::rt::{self, set_timeout, spawn, yield_now, Deferred, Timer};
use crate::re;
use crate::tmux::tmux::{capture_panes, send_keys};

const PAGE: usize = 160;
/// messages kept per open chat; older ones stay in the transcript file (and the terminal)
const KEEP: usize = 4000;
/// fallback screen poll; tmux control mode triggers re-reads as soon as the pane changes
const SCREEN_MS: u64 = 2000;
/// while the phone app is in the background nobody sees the screen: read it rarely
const QUIET_SCREEN_MS: u64 = 20_000;
const IDLE_SHUTDOWN_MS: u64 = 120_000;
/// how long a reply read off the screen stays after its turn, waiting for its record
const SCREEN_LIVE_WAIT_MS: u64 = 5000;
/// agents whose input box Inyon can read and retype, so a draft follows you between app and terminal
const MIRRORS: [AgentKind; 5] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity, AgentKind::Opencode, AgentKind::Copilot];

/// a closed chat's state is kept this long, so opening it again carries on instead of re-reading
const PARK_MS: u64 = 30 * 60_000;
const PARKED_MAX: usize = 6;
/// more than this to read on: catch up out of sight and show the result at once
const CATCHUP_BYTES: u64 = 64 * 1024;
/// a read that hasn't reached the end by then is shown as far as it got, if nothing more arrives
const FIRST_LINE_MS: u64 = 8000;
/// A long Claude conversation opens on its last stretch of file, and earlier messages are read
/// backwards, a piece at a time, as you scroll up. Messages read that way are numbered down from
/// TAIL_SEQ, so they still sort before the ones after them. The stretch is small so the chat shows
/// quickly; a page is filled in from before it once it's on screen.
const TAIL_BYTES: u64 = 256_000;
const OLDER_BYTES: u64 = 1_000_000;
const TAIL_SEQ: i64 = 1_000_000_000;
const QUIET_MS: u64 = 5000;

thread_local! {
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
    static WATCHERS: Watchers = Watchers::default();
}

fn hidden() -> bool {
    HIDDEN.with(Cell::get)
}

fn debug() -> bool {
    console::debug()
}

/// Which agent process a pane runs: its pid and when it started. A conversation belongs to one, so
/// a new agent in the same pane (Claude exited and started again) never shows the last one's chat.
fn proc_of(a: Option<&AgentPaneInfo>) -> Option<String> {
    let a = a.filter(|a| a.pid != 0)?;
    Some(format!("{}:{}", a.pid, a.started_at.map(|s| s.to_string()).unwrap_or_default()))
}

/// What a stopped watcher leaves behind for the next one on the same pane.
#[derive(Clone)]
pub struct Parked {
    at: u64,
    transcript: String,
    proc: Option<String>,
    reader: TranscriptReader,
    seq: i64,
    codex: bool,
    agent: Option<AgentKind>,
    items: Vec<ChatItem>,
    meta: TranscriptMeta,
    info: ChatInfo,
    older: Option<u64>,
}

/// Someone watching a chat (a phone, a browser tab).
pub trait Subscriber {
    fn send(&self, msg: &ServerMsg);
}

pub type Sub = Rc<dyn Subscriber>;

fn same_sub(a: &Sub, b: &Sub) -> bool {
    std::ptr::eq(Rc::as_ptr(a) as *const (), Rc::as_ptr(b) as *const ())
}

fn blank_screen() -> ScreenState {
    serde_json::from_value(serde_json::json!({ "working": false, "width": 0 })).expect("a blank screen")
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).map(|v| stringify(&v)).unwrap_or_default()
}

/// A permission OpenCode asked for, answered through its API.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct ApiPermission {
    id: String,
    base: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// a question OpenCode's agent asked, answered through its API one question at a time
struct ApiQuestion {
    flow: OpenCodeQuestionFlow,
    client: Rc<OpenCodeClient>,
}

/// Formats rewritten per step (Gemini's session file, Antigravity's transcript).
trait Doc {
    fn apply(&mut self, rec: &Value);
    fn items(&self, meta: &mut TranscriptMeta) -> Vec<ChatItem>;
}

impl Doc for GeminiDoc {
    fn apply(&mut self, rec: &Value) {
        GeminiDoc::apply(self, rec)
    }
    fn items(&self, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
        GeminiDoc::items(self, meta)
    }
}

impl Doc for AntigravityDoc {
    fn apply(&mut self, rec: &Value) {
        AntigravityDoc::apply(self, rec)
    }
    fn items(&self, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
        AntigravityDoc::items(self, meta)
    }
}

struct State {
    subs: Vec<Sub>,
    items: Vec<ChatItem>,
    meta: TranscriptMeta,
    agent: Option<AgentKind>,
    screen: ScreenState,
    info: ChatInfo,
    tail: Option<StreamHandle>,
    /// the transcript tail ended by itself (the machine dropped off): follow again once it's back
    tail_lost: bool,
    transcript: Option<String>,
    /// the agent process whose conversation is shown (proc_of)
    proc: Option<String>,
    /// how far into the transcript file it has read, so a dropped tail can carry on from there
    reader: Option<TranscriptReader>,
    seq: i64,
    /// file offset of the line the first held message came from; 0 when nothing is before it
    older_from: Option<u64>,
    /// each held message's line in the file, so trimming from the front knows where it cut
    line_at: HashMap<i64, u64>,
    /// opened on the last stretch and no line read yet: the first one read is where older ones end
    tail_start: bool,
    /// Lines read since a long conversation opened, while the summary of the part before them is
    /// still on its way. Once it lands they're replayed onto it, so it ends up as a full read's.
    /// The number tells one replay from the next (the TS compares the arrays).
    replay: Option<(u64, Vec<Value>)>,
    replays: u64,
    codex: bool,
    /// A Codex thread rewound earlier continues an older segment file. Opened on its last stretch,
    /// those earlier lines wait here until scrolling back reaches the start of this file.
    held_history: Option<Vec<String>>,
    ready: bool,
    /// Reading a backlog (what was said while nobody watched): updates are held back and one reset
    /// brings the result, so the chat doesn't replay hours of messages and passing states (a goal
    /// since met) don't flash by. Meanwhile new subscribers get the chat as it was before.
    behind: Option<ServerMsg>,
    /// A saved (or parked) copy's goal may have been met or cleared since. Until what was written
    /// in the meantime is read, no goal shows, so an old one doesn't flash up and vanish.
    goal_unsure: bool,
    history: Option<Deferred<()>>,
    pending: Vec<ChatItem>,
    flush_timer: Option<Timer>,
    screen_timer: Option<Timer>,
    idle_timer: Option<Timer>,
    stopped: bool,
    screen_json: String,
    /// agy Remote Control, as last seen on screen
    remote: Option<RemoteControl>,
    generation: u64,
    /// Codex's reply as it's being written, from its app-server's live events (codex_live.rs)
    live: CodexLiveState,
    /// the reply Claude Code, Copilot or Antigravity is writing, read off the screen (screen_reply.rs)
    screen_live: Option<LiveItem>,
    live_tail: Option<StreamHandle>,
    live_timer: Option<Timer>,
    live_expiry: Option<Timer>,
    /// how long the screen's copy of a reply waits for its record once the turn is over
    screen_live_expiry: Option<Timer>,
    live_retry_timer: Option<Timer>,
    live_retries: u32,
    live_json: String,
    /// when a question Codex asked mid-turn was last opened on its screen
    question_opened_at: u64,
    cache_timer: Option<Timer>,
    mirror_timer: Option<Timer>,
    mirroring_until: u64,
    /// a send owns the input box until then; no mirroring meanwhile
    sending_until: u64,
    mirroring: Option<Deferred<()>>,
    /// the last message sent from here, and when, to tell a stale draft of it from a new one
    last_sent: Option<(String, u64)>,
    ledger: SendLedger,
    progress: Option<ServerMsg>,
    progress_at: u64,
    snapshot_timer: Option<Timer>,
    api_permission: Option<ApiPermission>,
    api_question: Option<ApiQuestion>,
    retry_timer: Option<Timer>,
    retries: u32,
    info_timer: Option<Timer>,
    change_timer: Option<Timer>,
    opencode_poll: Option<Timer>,
}

pub struct ChatWatcher {
    pub key: String,
    pub host_id: String,
    pub pane_id: String,
    me: Weak<ChatWatcher>,
    st: RefCell<State>,
}

/// Schedules a refresh of an OpenCode conversation read over its API.
type Later = Rc<dyn Fn(&Rc<ChatWatcher>)>;

/// Shared by the steps of one initial read in [`ChatWatcher::follow`].
struct Initial {
    gen: u64,
    total: u64,
    remaining: Cell<i64>,
    filling: Cell<bool>,
    quiet: RefCell<Option<Timer>>,
    t0: u64,
}

impl ChatWatcher {
    fn new(host_id: &str, pane_id: &str, parked: Option<Parked>) -> Rc<ChatWatcher> {
        let key = chat_key(host_id, pane_id);
        let w = Rc::new_cyclic(|me| ChatWatcher {
            key,
            host_id: host_id.to_owned(),
            pane_id: pane_id.to_owned(),
            me: me.clone(),
            st: RefCell::new(State {
                subs: vec![],
                items: vec![],
                meta: new_meta(),
                agent: None,
                screen: blank_screen(),
                info: empty_info(host_id, pane_id),
                tail: None,
                tail_lost: false,
                transcript: None,
                proc: None,
                reader: None,
                seq: 0,
                older_from: None,
                line_at: HashMap::new(),
                tail_start: false,
                replay: None,
                replays: 0,
                codex: false,
                held_history: None,
                ready: false,
                behind: None,
                goal_unsure: false,
                history: None,
                pending: vec![],
                flush_timer: None,
                screen_timer: None,
                idle_timer: None,
                stopped: false,
                screen_json: String::new(),
                remote: None,
                generation: 0,
                live: CodexLiveState::new(),
                screen_live: None,
                live_tail: None,
                live_timer: None,
                live_expiry: None,
                screen_live_expiry: None,
                live_retry_timer: None,
                live_retries: 0,
                live_json: String::new(),
                question_opened_at: 0,
                cache_timer: None,
                mirror_timer: None,
                mirroring_until: 0,
                sending_until: 0,
                mirroring: None,
                last_sent: None,
                ledger: SendLedger::new(),
                progress: None,
                progress_at: 0,
                snapshot_timer: None,
                api_permission: None,
                api_question: None,
                retry_timer: None,
                retries: 0,
                info_timer: None,
                change_timer: None,
                opencode_poll: None,
            }),
        });
        match parked {
            Some(p) => w.unpark(p),
            None => w.load_cached(),
        }
        w.sync();
        w.kick_screen();
        w
    }

    fn rc(&self) -> Rc<ChatWatcher> {
        self.me.upgrade().expect("a live watcher")
    }

    /// Runs `f` on this watcher later, if it still exists.
    fn later(&self, ms: u64, f: impl FnOnce(&Rc<ChatWatcher>) + 'static) -> Timer {
        let me = self.me.clone();
        set_timeout(ms, move || {
            if let Some(w) = me.upgrade() {
                f(&w)
            }
        })
    }

    /// `void this.pollScreen()`
    pub(crate) fn kick_screen(&self) {
        let w = self.rc();
        spawn(async move { w.poll_screen().await });
    }

    /// The transcript tail ends as a dropped connection would end it (tests).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn kill_tail(&self) {
        let t = self.st.borrow().tail.clone();
        if let Some(t) = t {
            t.kill();
        }
    }

    /// Its tail ended by itself and waits to be followed again (tests).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn tail_lost(&self) -> bool {
        self.st.borrow().tail_lost
    }

    /// Moves where it thinks it stopped reading, as a file replaced under it would (tests).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn shift_reader(&self, by: i64) {
        if let Some(r) = self.st.borrow_mut().reader.as_mut() {
            r.offset = (r.offset as i64 + by).max(0) as u64;
        }
    }

    pub fn items(&self) -> Vec<ChatItem> {
        self.st.borrow().items.clone()
    }

    pub fn screen(&self) -> ScreenState {
        self.st.borrow().screen.clone()
    }

    pub fn info(&self) -> ChatInfo {
        self.st.borrow().info.clone()
    }

    pub fn subscribers(&self) -> usize {
        self.st.borrow().subs.len()
    }

    fn on_remote_host(&self) -> bool {
        registry().get(&self.host_id).map(|r| r.borrow().host.config().kind) != Some(HostKind::Local)
    }

    fn online(&self) -> bool {
        registry().get(&self.host_id).is_some_and(|r| r.borrow().state.status == HostStatus::Online)
    }

    fn host(&self) -> HubResult<Host> {
        registry().host(&self.host_id)
    }

    /// The saved copy of this conversation: when it says where reading stopped (and the pane still
    /// has that transcript) carry on from there. Otherwise it's only shown while the machine isn't
    /// answering, if it's this session.
    fn load_cached(&self) {
        let Some(cached) = load_chat(&self.key) else { return };
        let agent = registry().pane_agent(&self.host_id, &self.pane_id);
        if agent.as_ref().and_then(|a| a.session_id.as_deref()).is_some_and(|s| !s.is_empty() && Some(s) != cached.session_id.as_deref()) {
            return;
        }
        // a Claude chat saved before long conversations opened on their last stretch doesn't say where
        // its first message is in the file; reading it afresh is quick now, scrolling up from it isn't
        let at = cached.at.clone();
        let outdated = at.as_ref().is_some_and(|a| !a.codex && a.agent != Some(AgentKind::Copilot) && a.older.is_none()) || at.as_ref().and_then(|a| a.reader) != Some(READER);
        let path_ok = agent.as_ref().and_then(|a| a.transcript_path.as_deref()).is_none_or(|p| p.is_empty() || Some(p) == cached.transcript.as_deref());
        if let (Some(at), false) = (&at, outdated) {
            if let Some(transcript) = cached.transcript.clone().filter(|t| !t.is_empty() && !cached.items.is_empty() && path_ok) {
                if let Some(meta) = load_meta(&at.meta) {
                    return self.unpark(Parked {
                        at: cached.saved_at,
                        transcript,
                        proc: cached.proc.clone(),
                        reader: TranscriptReader::new(at.offset),
                        seq: at.seq,
                        codex: at.codex,
                        agent: at.agent,
                        items: cached.items.clone(),
                        meta,
                        info: cached.info.clone(),
                        older: at.older,
                    });
                }
            }
        }
        if registry().get(&self.host_id).is_none() || self.online() || !self.on_remote_host() {
            return;
        }
        let mut st = self.st.borrow_mut();
        st.items = cached.items;
        st.proc = cached.proc;
        // without the session id the copy may be an older conversation in this pane: show what was
        // said, but not a goal or queue that may be long gone
        let sure = agent.as_ref().and_then(|a| a.session_id.as_deref()).is_some_and(|s| !s.is_empty());
        let mut info = cached.info;
        info.running = agent.is_some();
        if !sure {
            info.goal = None;
            info.queue = vec![];
        }
        st.info = info;
        st.ready = true;
    }

    /// Picks up where the last watcher on this pane stopped; sync() then follows the file from there.
    fn unpark(&self, p: Parked) {
        let mut st = self.st.borrow_mut();
        st.transcript = Some(p.transcript);
        st.proc = p.proc;
        st.reader = Some(p.reader);
        st.seq = p.seq;
        st.codex = p.codex;
        st.agent = p.agent;
        st.items = p.items;
        st.meta = p.meta;
        st.info = ChatInfo { goal: None, ..p.info };
        st.older_from = p.older;
        st.goal_unsure = true;
        st.ready = true;
        st.tail_lost = true;
    }

    fn park(&self) -> Option<Parked> {
        let st = self.st.borrow();
        if !st.ready {
            return None;
        }
        let (Some(reader), Some(transcript)) = (st.reader.clone(), st.transcript.clone()) else { return None };
        Some(Parked {
            at: now_ms(),
            transcript,
            proc: st.proc.clone(),
            reader,
            seq: st.seq,
            codex: st.codex,
            agent: st.agent,
            items: st.items.clone(),
            meta: st.meta.clone(),
            info: st.info.clone(),
            older: st.older_from,
        })
    }

    /// Keeps the saved copy of the conversation current (at most every 10 s).
    fn cache_later(&self) {
        if self.st.borrow().cache_timer.is_some() {
            return;
        }
        let t = self.later(10_000, |w| {
            let (stopped, empty, ready) = {
                let mut st = w.st.borrow_mut();
                st.cache_timer = None;
                (st.stopped, st.items.is_empty(), st.ready)
            };
            if stopped || empty {
                return;
            }
            // still loading: try again once the whole conversation is in
            if !ready {
                return w.cache_later();
            }
            w.save();
        });
        self.st.borrow_mut().cache_timer = Some(t);
    }

    pub fn save(&self) {
        // a summary still missing the part before the last stretch would be saved as if it were
        // whole: wait for it
        if self.st.borrow().replay.is_some() {
            return self.cache_later();
        }
        let chat = {
            let st = self.st.borrow();
            if !st.ready || st.items.is_empty() {
                return;
            }
            // the read position is saved only with the summary and messages read up to it
            // only the latest messages are saved: cut between lines and remember where the first one starts
            let n = st.items.len();
            let mut first = n.saturating_sub(CACHE_KEEP);
            let mut older = st.older_from;
            if first > 0 && older.is_some() {
                let line = |i: usize| st.line_at.get(&st.items[i].seq()).copied();
                // messages restored from a saved copy have no known line, so the cut moves on past them too;
                // with none known the copy is saved without a place and read afresh next time
                while first < n && (line(first).is_none() || line(first) == line(first - 1)) {
                    first += 1;
                }
                older = if first < n { line(first) } else { None };
                if older.is_none() {
                    first = n.saturating_sub(CACHE_KEEP);
                }
            }
            let at = match (&st.reader, &st.transcript) {
                (Some(r), Some(_)) => Some(ChatAt { offset: r.offset, seq: st.seq, codex: st.codex, agent: st.agent, meta: save_meta(&st.meta), older, reader: Some(READER) }),
                _ => None,
            };
            (at, first, st.info.clone(), st.transcript.clone(), st.proc.clone(), st.items[first..].to_vec())
        };
        let (at, _, info, transcript, proc, items) = chat;
        if at.is_none() && !self.on_remote_host() {
            return;
        }
        save_chat(&self.key, CachedChat { session_id: info.session_id.clone(), transcript, proc, info, items, saved_at: 0, at });
    }

    pub fn subscribe(&self, s: Sub) {
        let msgs = {
            let mut st = self.st.borrow_mut();
            rt::clear(&mut st.idle_timer);
            if !st.subs.iter().any(|x| same_sub(x, &s)) {
                st.subs.push(s.clone());
            }
            let mut msgs = vec![];
            if let Some(b) = &st.behind {
                msgs.push(b.clone());
            } else if st.ready {
                msgs.push(reset_msg(&self.key, &st));
            } else {
                msgs.push(ServerMsg::ChatInfo { key: self.key.clone(), info: Box::new(st.info.clone()) });
            }
            if let Some(p) = &st.progress {
                msgs.push(p.clone());
            }
            msgs.push(ServerMsg::ChatScreen { key: self.key.clone(), screen: Box::new(st.screen.clone()) });
            let live = live_items(&st);
            if !live.is_empty() {
                msgs.push(ServerMsg::ChatLive { key: self.key.clone(), items: live });
            }
            msgs
        };
        for m in msgs {
            s.send(&m);
        }
        if let Some(d) = get_draft(&self.key) {
            s.send(&ServerMsg::ChatDraft { key: self.key.clone(), text: d.text, source: d.source, at: d.at as u64 });
        }
    }

    // ---------- drafts (two-way with Claude's input box) ----------

    fn broadcast_draft(&self, except: Option<&Sub>) {
        let d = get_draft(&self.key);
        let msg = ServerMsg::ChatDraft {
            key: self.key.clone(),
            text: d.as_ref().map(|d| d.text.clone()).unwrap_or_default(),
            source: d.as_ref().map_or(DraftSource::App, |d| d.source),
            at: d.as_ref().map_or(now_ms(), |d| d.at as u64),
        };
        let subs = self.st.borrow().subs.clone();
        for s in subs {
            if except.is_none_or(|e| !same_sub(e, &s)) {
                s.send(&msg);
            }
        }
    }

    /// Text typed in the app: save it, share it with other devices, and mirror it into the terminal.
    pub fn draft_from_app(&self, text: &str, from: Option<&Sub>) {
        // A phone's keyboard can put a message back in the box just after it was sent, and the app
        // passes that on as a draft. It isn't one: keep the box empty, and tell that app so.
        let ls = self.st.borrow().last_sent.clone();
        if let Some((sent, at)) = ls {
            if now_ms().saturating_sub(at) < 5000 && !squash(text).is_empty() && squash(&sent).ends_with(&squash(text)) {
                let d = get_draft(&self.key);
                if let Some(from) = from {
                    from.send(&ServerMsg::ChatDraft {
                        key: self.key.clone(),
                        text: d.as_ref().map(|d| d.text.clone()).unwrap_or_default(),
                        source: d.as_ref().map_or(DraftSource::App, |d| d.source),
                        at: now_ms(),
                    });
                }
                return;
            }
        }
        let prev = get_draft(&self.key);
        set_draft(&self.key, Some(Draft { text: text.to_owned(), source: DraftSource::App, at: now_ms() as f64, mirrored: prev.and_then(|p| p.mirrored) }));
        self.broadcast_draft(from);
        let t = self.later(900, |w| w.mirror());
        let mut st = self.st.borrow_mut();
        rt::clear(&mut st.mirror_timer);
        st.mirror_timer = Some(t);
    }

    fn mirror(&self) {
        let until = self.st.borrow().sending_until;
        // read once: the clock can tick between two reads
        let now = now_ms();
        if now < until {
            let t = self.later(until - now + 100, |w| w.mirror());
            let mut st = self.st.borrow_mut();
            rt::clear(&mut st.mirror_timer);
            st.mirror_timer = Some(t);
            return;
        }
        let d = Deferred::new();
        self.st.borrow_mut().mirroring = Some(d.clone());
        let w = self.rc();
        spawn(async move {
            w.do_mirror().await;
            w.st.borrow_mut().mirroring = None;
            d.resolve(());
        });
    }

    /// A message is about to be typed and submitted: drop any pending mirror and wait for one in
    /// flight, or both type into the box and the message goes in twice.
    pub async fn begin_send(&self) {
        let m = {
            let mut st = self.st.borrow_mut();
            rt::clear(&mut st.mirror_timer);
            st.sending_until = now_ms() + 15_000;
            st.mirroring.clone()
        };
        if let Some(m) = m {
            m.wait().await;
        }
    }

    /// A settings command borrows the input box (it clears it, types "/model", opens a menu): keep
    /// the draft as it was, ignore what the box shows meanwhile, and put the draft back afterwards.
    pub async fn hold_draft(&self, on_screen: Option<&str>) -> impl FnOnce() -> LocalBoxFuture<'static, ()> + use<> {
        rt::clear(&mut self.st.borrow_mut().mirror_timer);
        let stop_driving = start_driving(&self.host_id, &self.pane_id);
        let m = self.st.borrow().mirroring.clone();
        if let Some(m) = m {
            m.wait().await;
        }
        if let Some(text) = on_screen.filter(|t| !t.is_empty()) {
            if get_draft(&self.key).is_none() {
                set_draft(&self.key, Some(Draft { text: text.to_owned(), source: DraftSource::Terminal, at: now_ms() as f64, mirrored: Some(text.to_owned()) }));
            }
        }
        {
            let mut st = self.st.borrow_mut();
            st.sending_until = now_ms() + 60_000;
            st.mirroring_until = st.sending_until;
        }
        let w = self.rc();
        move || {
            Box::pin(async move {
                stop_driving();
                // read the screen as it is now (the menu is gone), so the empty box isn't taken for the
                // draft and the mirror doesn't skip because it still sees the menu
                w.poll_screen().await;
                w.st.borrow_mut().sending_until = 0;
                if let Some(d) = get_draft(&w.key) {
                    let draft = w.st.borrow().screen.draft.clone().unwrap_or_default();
                    w.st.borrow_mut().mirroring_until = now_ms() + 2500;
                    set_draft(&w.key, Some(Draft { mirrored: Some(draft), ..d }));
                    w.mirror();
                    let m = w.st.borrow().mirroring.clone();
                    if let Some(m) = m {
                        m.wait().await;
                    }
                }
                w.st.borrow_mut().mirroring_until = now_ms() + 800;
            }) as LocalBoxFuture<'static, ()>
        }
    }

    async fn do_mirror(&self) {
        let (stopped, prompt, input_visible, current) = {
            let st = self.st.borrow();
            (st.stopped, st.screen.prompt.is_some(), st.screen.input_visible == Some(true), st.screen.draft.clone().unwrap_or_default())
        };
        if stopped {
            return;
        }
        let kind = registry().pane_agent(&self.host_id, &self.pane_id).map(|a| a.kind);
        let d = get_draft(&self.key);
        if !kind.is_some_and(|k| MIRRORS.contains(&k)) || prompt || !input_visible {
            return;
        }
        let target = d.as_ref().map(|d| d.text.clone()).unwrap_or_default();
        // a paste preview in the box is this draft only if the mirror put this draft there
        let known = squash(d.as_ref().and_then(|d| d.mirrored.as_deref()).unwrap_or("")) == squash(&target);
        if squash(&current) == squash(&target) || (!target.is_empty() && input_contains(Some(&current), &target, known)) {
            if let Some(d) = d {
                set_draft(&self.key, Some(Draft { mirrored: Some(target), ..d }));
            }
            return;
        }
        self.st.borrow_mut().mirroring_until = now_ms() + 2500;
        if let Some(d) = d {
            set_draft(&self.key, Some(Draft { mirrored: Some(target.clone()), ..d }));
        }
        let _ = mirror_to_terminal(&self.host_id, &self.pane_id, &target, Some(&current)).await;
        self.st.borrow_mut().mirroring_until = now_ms() + 800;
        self.kick_screen();
    }

    /// Called after a message is sent: the box is empty again everywhere. A message from the app's
    /// box rarely matches the saved draft exactly (edits are pushed on a delay, attachments add
    /// their paths), so for those (`started_at` set) only a draft written after the send began
    /// survives. Picker commands leave `started_at` at 0 and keep a half-typed draft.
    pub fn draft_sent(&self, text: Option<&str>, started_at: u64) {
        let text = text.filter(|t| !t.is_empty());
        {
            let mut st = self.st.borrow_mut();
            if let Some(t) = text {
                st.last_sent = Some((t.to_owned(), now_ms()));
            }
            rt::clear(&mut st.mirror_timer);
            st.sending_until = now_ms() + 1500;
        }
        let current = get_draft(&self.key);
        // with a start time, a draft written since survives unless it is this message (pushed just
        // before the send, arriving just after), or this message before its attached files were added
        let drop = match (text, &current) {
            (Some(t), Some(c)) => {
                let sent = squash(t) == squash(&c.text) || squash(t).starts_with(&format!("{}Attachedfiles:", squash(&c.text)));
                if started_at != 0 {
                    c.at <= started_at as f64 || sent
                } else {
                    sent || squash(t).starts_with(&squash(&c.text))
                }
            }
            _ => true,
        };
        if drop {
            set_draft(&self.key, None);
        } else {
            let t = self.later(1600, |w| w.mirror());
            self.st.borrow_mut().mirror_timer = Some(t);
        }
        self.broadcast_draft(None);
    }

    /// Its first page is loaded (from a transcript, or an agent's own server).
    pub fn ready(&self) -> bool {
        self.st.borrow().ready
    }

    /// Following the agent's transcript, so `recorded` can be trusted.
    pub fn following(&self) -> bool {
        let st = self.st.borrow();
        st.ready && st.transcript.is_some() && st.tail.is_some()
    }

    // ---------- sends (what reached the agent) ----------

    pub fn track_send(&self, id: &str, text: &str, setting: bool) {
        {
            let mut st = self.st.borrow_mut();
            let State { ledger, items, .. } = &mut *st;
            ledger.track(id, text, items, setting);
        }
        self.publish_info();
    }

    pub fn settle_send(&self, id: &str, state: Option<SendStatus>, error: Option<String>) {
        self.st.borrow_mut().ledger.settle(id, state, error);
        self.publish_info();
    }

    /// An item the agent wrote, put straight in (the TS tests push onto `w.items`).
    #[cfg(test)]
    pub(crate) fn push_item(&self, item: ChatItem) {
        self.st.borrow_mut().items.push(item);
    }

    pub fn delivery(&self, id: &str) -> Option<inyon_protocol::SendState> {
        let mut st = self.st.borrow_mut();
        let queue = current_queue(&st);
        let State { ledger, items, .. } = &mut *st;
        ledger.match_items(items, &queue);
        ledger.get(id)
    }

    /// The terminal's input box changed: if someone typed there, adopt it as the draft.
    fn sync_draft_from_screen(&self) {
        let term = {
            let st = self.st.borrow();
            if now_ms() < st.mirroring_until || now_ms() < st.sending_until || st.screen.prompt.is_some() || st.screen.input_visible != Some(true) {
                return;
            }
            st.screen.draft.clone().unwrap_or_default()
        };
        if has_paste_preview(&term) {
            return;
        }
        let d = get_draft(&self.key);
        if squash(&term) == squash(d.as_ref().and_then(|d| d.mirrored.as_deref()).unwrap_or("")) {
            return;
        }
        if squash(&term) == squash(d.as_ref().map_or("", |d| d.text.as_str())) {
            if let Some(d) = d {
                set_draft(&self.key, Some(Draft { mirrored: Some(term), ..d }));
            }
            return;
        }
        set_draft(&self.key, (!term.is_empty()).then(|| Draft { text: term.clone(), source: DraftSource::Terminal, at: now_ms() as f64, mirrored: Some(term.clone()) }));
        self.broadcast_draft(None);
    }

    /// Used without anyone watching (a send from elsewhere): stop later unless someone subscribes.
    pub fn idle_later(&self) {
        if !self.st.borrow().subs.is_empty() {
            return;
        }
        let key = self.key.clone();
        let t = set_timeout(IDLE_SHUTDOWN_MS, move || watchers().stop(&key));
        let mut st = self.st.borrow_mut();
        rt::clear(&mut st.idle_timer);
        st.idle_timer = Some(t);
    }

    pub fn unsubscribe(&self, s: &Sub) {
        let mut st = self.st.borrow_mut();
        st.subs.retain(|x| !same_sub(x, s));
        if st.subs.is_empty() {
            let key = self.key.clone();
            // the TS leaves an earlier idle timer running beside this one; both stop the same watcher
            st.idle_timer = Some(set_timeout(IDLE_SHUTDOWN_MS, move || watchers().stop(&key)));
        }
    }

    /// A very long session keeps only its latest messages in memory (in chunks, not on every push).
    fn trim(&self) {
        {
            let mut st = self.st.borrow_mut();
            if st.items.len() > KEEP + 500 {
                let n = st.items.len();
                let mut cut = n - KEEP;
                if st.older_from.is_some() {
                    // cut between lines, never inside one, and only where the line is known (or the read
                    // back from the file couldn't join up): otherwise keep everything a while longer
                    let line = |i: usize| st.line_at.get(&st.items[i].seq()).copied();
                    while cut < n && line(cut).is_some() && line(cut) == line(cut - 1) {
                        cut += 1;
                    }
                    let at = if cut < n { line(cut) } else { None };
                    match at {
                        None => {
                            drop(st);
                            return self.cache_later();
                        }
                        Some(at) => st.older_from = Some(at),
                    }
                }
                let gone: Vec<ChatItem> = st.items.drain(..cut).collect();
                for it in gone {
                    st.line_at.remove(&it.seq());
                }
            }
        }
        self.cache_later();
    }

    pub async fn more(&self, s: &Sub, before: i64) {
        let find = |st: &State| st.items.iter().position(|i| i.seq() >= before);
        let idx = find(&self.st.borrow());
        let idx = if idx == Some(0) && self.older_in_file() {
            if self.st.borrow().older_from.is_some() {
                self.load_older().await;
            } else {
                self.load_history().await;
            }
            find(&self.st.borrow())
        } else {
            idx
        };
        let msg = {
            let st = self.st.borrow();
            let end = idx.unwrap_or(st.items.len());
            let start = end.saturating_sub(PAGE);
            ServerMsg::ChatMore { key: self.key.clone(), items: st.items[start..end].to_vec(), has_more: start > 0 || older_in_file(&st) }
        };
        s.send(&msg);
    }

    /// Messages before the first one held (a saved copy, or a long session trimmed) are still in the file.
    fn older_in_file(&self) -> bool {
        older_in_file(&self.st.borrow())
    }

    /// One line of the agent's record → chat items, by the agent whose record it is.
    fn normalize_entry(codex: bool, agent: Option<AgentKind>, e: &Value, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
        if codex {
            normalize_codex(e, meta)
        } else if agent == Some(AgentKind::Copilot) {
            normalize_copilot(e, meta)
        } else {
            normalize(e, meta, false)
        }
    }

    fn kind(&self) -> (bool, Option<AgentKind>) {
        let st = self.st.borrow();
        (st.codex, st.agent)
    }

    /// Reads lines between two offsets of the transcript, each with the offset its line starts at.
    /// `complete` says every line up to `to` arrived: a dropped connection or a timeout ends the
    /// stream early, and taking that for the whole piece would leave a gap nothing reads again.
    async fn read_range(&self, path: &str, from: u64, to: u64) -> (Vec<(Value, u64)>, bool) {
        let Ok(host) = self.host() else { return (vec![], false) };
        let reader = Rc::new(RefCell::new(TranscriptReader::new(from)));
        let out: Rc<RefCell<Vec<(Value, u64)>>> = Rc::default();
        let (r2, o2) = (reader.clone(), out.clone());
        let stream = host.stream(
            &range_command(path, from, to, self.on_remote_host()),
            move |line| {
                let mut reader = r2.borrow_mut();
                if let Some(r) = reader.line(&line) {
                    if let Some(entry) = r.entry {
                        o2.borrow_mut().push((entry, reader.offset - r.bytes));
                    }
                }
            },
            false,
        );
        let code = match stream {
            Ok(t) => wait_stream(&t, 60_000, -1).await,
            Err(_) => -1,
        };
        let received = reader.borrow().received();
        let lines = std::mem::take(&mut *out.borrow_mut());
        (lines, code == 0 && received == to)
    }

    /// What's known about a conversation as a whole (title, goal, counts, tasks, subagents) from
    /// the part before its last stretch: only the lines that carry it are sent, and what they say
    /// goes into the summary, not the chat.
    async fn read_meta(&self, path: &str, to: u64, meta: Rc<RefCell<TranscriptMeta>>, from: u64) {
        let Ok(host) = self.host() else { return };
        let (codex, agent) = self.kind();
        let mut reader = TranscriptReader::new(from);
        let cmd = if codex { codex_meta_command(path, to, self.on_remote_host(), from) } else { meta_command(path, to, self.on_remote_host(), from) };
        let stream = host.stream(
            &cmd,
            move |line| {
                if let Some(entry) = reader.line(&line).and_then(|r| r.entry) {
                    Self::normalize_entry(codex, agent, &entry, &mut meta.borrow_mut());
                }
            },
            false,
        );
        if let Ok(t) = stream {
            wait_stream(&t, 30_000, 0).await;
        }
    }

    /// `this.history ??= work().finally(() => this.history = undefined)`
    fn history(&self, work: impl FnOnce(Rc<ChatWatcher>) -> LocalBoxFuture<'static, ()>) -> impl Future<Output = ()> + 'static {
        if let Some(h) = self.st.borrow().history.clone() {
            return h.wait();
        }
        let d = Deferred::new();
        self.st.borrow_mut().history = Some(d.clone());
        let w = self.rc();
        let fut = work(w.clone());
        let done = d.clone();
        spawn(async move {
            fut.await;
            w.st.borrow_mut().history = None;
            done.resolve(());
        });
        d.wait()
    }

    /// Earlier messages of a conversation opened on its last stretch: reads the file backwards from
    /// the first held message, a piece at a time, until there's a page of them or the file begins.
    fn load_older(&self) -> impl Future<Output = ()> + 'static {
        self.history(|w| Box::pin(async move { w.load_older_now().await }))
    }

    async fn load_older_now(&self) {
        let (path, first, mut end) = {
            let st = self.st.borrow();
            let Some(path) = st.transcript.clone() else { return };
            (path, st.items.first().map_or(TAIL_SEQ, ChatItem::seq), st.older_from.unwrap_or(0))
        };
        let (codex, agent) = self.kind();
        let mut older: Vec<(ChatItem, u64)> = vec![];
        let mut span = OLDER_BYTES;
        while end > 0 && older.len() < PAGE {
            let from = end.saturating_sub(span);
            let (lines, complete) = self.read_range(&path, from, end).await;
            if self.gone(&path) {
                return;
            }
            // nothing is kept from a read cut short: the next scroll up asks for the same piece again
            if !complete {
                return;
            }
            if lines.is_empty() {
                // one line longer than the piece (a screenshot): take a bigger piece
                if from == 0 {
                    end = 0;
                } else {
                    span *= 4;
                }
                continue;
            }
            // tool calls and their results sit close together, so a piece's own summary links them
            let mut meta = new_meta();
            let mut got = vec![];
            for (entry, at) in &lines {
                for d in Self::normalize_entry(codex, agent, entry, &mut meta) {
                    got.push((d, *at));
                }
            }
            got.append(&mut older);
            older = got;
            end = lines[0].1;
        }
        // the start of a Codex segment file: what the thread said before its rewind comes first
        let held = if end == 0 { self.st.borrow_mut().held_history.take().filter(|h| !h.is_empty()) } else { None };
        if let Some(held) = held {
            let mut meta = new_meta();
            let mut got = vec![];
            for line in held {
                if let Some(e) = json::parse(&line) {
                    for d in normalize_codex(&e, &mut meta) {
                        got.push((d, 0));
                    }
                }
            }
            got.append(&mut older);
            older = got;
        }
        let mut st = self.st.borrow_mut();
        if st.stopped || st.transcript.as_deref() != Some(path.as_str()) || st.items.first().map_or(TAIL_SEQ, ChatItem::seq) != first {
            return;
        }
        // below 0 when scrolling back past where numbering started, as the TS goes negative
        let start = first - older.len() as i64;
        let mut items = Vec::with_capacity(older.len());
        for (seq, (item, at)) in (start..).zip(older) {
            st.line_at.insert(seq, at);
            items.push(with_seq(item, seq));
        }
        items.append(&mut st.items);
        st.items = items;
        st.older_from = Some(end);
        st.tail_start = false;
    }

    /// The watcher stopped or moved to another file while a read was out.
    fn gone(&self, path: &str) -> bool {
        let st = self.st.borrow();
        st.stopped || st.transcript.as_deref() != Some(path)
    }

    /// Reads the transcript from the start for the messages before the first one held. They're
    /// numbered the same way as on the first read, so they join up with the rest.
    fn load_history(&self) -> impl Future<Output = ()> + 'static {
        self.history(|w| Box::pin(async move { w.load_history_now().await }))
    }

    async fn load_history_now(&self) {
        let (path, first) = {
            let st = self.st.borrow();
            let Some(path) = st.transcript.clone() else { return };
            (path, st.items.first().map_or(0, ChatItem::seq))
        };
        let Ok(host) = self.host() else { return };
        let (codex, agent) = self.kind();
        struct Acc {
            meta: TranscriptMeta,
            older: Vec<ChatItem>,
            seq: i64,
        }
        let acc = Rc::new(RefCell::new(Acc { meta: new_meta(), older: vec![], seq: 0 }));
        let add = {
            let acc = acc.clone();
            move |e: &Value| -> bool {
                let mut a = acc.borrow_mut();
                let Acc { meta, older, seq } = &mut *a;
                for d in Self::normalize_entry(codex, agent, e, meta) {
                    if *seq >= first {
                        return false;
                    }
                    older.push(with_seq(d, *seq));
                    *seq += 1;
                }
                true
            }
        };
        if codex {
            for line in codex_history(&host, &path, 0).await.unwrap_or_default() {
                if let Some(e) = json::parse(&line) {
                    add(&e);
                }
            }
        }
        let mut reader = TranscriptReader::new(0);
        let killer: Rc<RefCell<Option<StreamHandle>>> = Rc::default();
        let stop = Deferred::<()>::new();
        let (k2, s2) = (killer.clone(), stop.clone());
        let stream = host.stream(
            &tail_command(&path, 0, self.on_remote_host(), false),
            move |line| {
                if let Some(e) = reader.line(&line).and_then(|r| r.entry) {
                    if !add(&e) {
                        if let Some(t) = k2.borrow().as_ref() {
                            t.kill();
                        }
                        s2.resolve(());
                    }
                }
            },
            false,
        );
        if let Ok(t) = stream {
            *killer.borrow_mut() = Some(t.clone());
            let watchdog = {
                let (t, s) = (t.clone(), stop.clone());
                set_timeout(120_000, move || {
                    t.kill();
                    s.resolve(());
                })
            };
            let exited = t.exited.clone();
            futures_util::future::select(Box::pin(exited.wait()), Box::pin(stop.wait())).await;
            watchdog.clear();
        }
        let older = std::mem::take(&mut acc.borrow_mut().older);
        let mut st = self.st.borrow_mut();
        if st.stopped || st.transcript.as_deref() != Some(path.as_str()) || st.items.first().map(ChatItem::seq) != Some(first) {
            return;
        }
        let keep = PAGE.max(KEEP.saturating_sub(st.items.len()));
        let mut items: Vec<ChatItem> = older[older.len().saturating_sub(keep)..].to_vec();
        items.append(&mut st.items);
        st.items = items;
    }

    /// How far a read that's out of sight has got (at most a few times a second).
    fn report_progress(&self, done: u64, total: u64, force: bool) {
        let msg = {
            let mut st = self.st.borrow_mut();
            if !force && now_ms().saturating_sub(st.progress_at) < 400 {
                return;
            }
            st.progress_at = now_ms();
            st.progress = (total > 0 && done < total).then(|| ServerMsg::ChatProgress { key: self.key.clone(), done, total });
            st.progress.clone().unwrap_or(ServerMsg::ChatProgress { key: self.key.clone(), done: 0, total: 0 })
        };
        self.broadcast(&msg);
    }

    fn broadcast(&self, msg: &ServerMsg) {
        let subs = self.st.borrow().subs.clone();
        for s in subs {
            s.send(msg);
        }
    }

    /// Re-reads pane info from the registry; restarts the tail if the session changed.
    pub fn sync(&self) {
        let found = registry().find_pane(&self.host_id, &self.pane_id);
        let agent = found.as_ref().and_then(|f| f.pane.agent.clone());
        if let Some(a) = &agent {
            self.st.borrow_mut().agent = Some(a.kind);
        }
        // a machine that isn't answering can't be read: keep what's shown (live or saved) until it's back
        let online = self.online();
        // a different agent in the pane with nothing written yet (a new Claude, say): its chat is empty,
        // not the conversation of the one before it
        let proc = proc_of(agent.as_ref());
        let path = agent.as_ref().and_then(|a| a.transcript_path.clone()).filter(|p| !p.is_empty());
        let (shown, held, tail_lost, transcript) = {
            let st = self.st.borrow();
            (st.proc.clone(), st.transcript.is_some() || !st.items.is_empty(), st.tail_lost, st.transcript.clone())
        };
        if online && agent.is_some() && path.is_none() && proc.is_some() && proc != shown && held {
            self.forget();
        }
        if let (true, Some(p)) = (online, &path) {
            if Some(p) != transcript.as_ref() || tail_lost {
                let (w, p) = (self.rc(), p.clone());
                spawn(async move { w.follow(p).await });
            }
        }
        {
            let mut st = self.st.borrow_mut();
            if st.transcript.is_none() && found.is_some() && path.is_none() {
                st.ready = true; // nothing to read; show the empty state
            }
        }
        self.publish_info();
    }

    /// Lets go of the conversation shown: the agent in the pane now hasn't said anything yet.
    fn forget(&self) {
        if let Some(t) = self.st.borrow_mut().tail.take() {
            t.kill();
        }
        self.stop_live();
        let msg = {
            let mut st = self.st.borrow_mut();
            st.generation += 1;
            st.transcript = None;
            st.reader = None;
            st.items = vec![];
            st.meta = new_meta();
            st.pending = vec![];
            st.behind = None;
            st.older_from = None;
            st.replay = None;
            st.goal_unsure = false;
            st.ready = true;
            st.line_at.clear();
            st.info = empty_info(&self.host_id, &self.pane_id);
            reset_msg(&self.key, &st)
        };
        self.broadcast(&msg);
    }

    /// What was written since the saved copy is read (and its summary is in): show the goal again.
    fn settle_goal(&self) {
        {
            let mut st = self.st.borrow_mut();
            if !st.goal_unsure || st.replay.is_some() {
                return;
            }
            st.goal_unsure = false;
        }
        self.publish_info();
    }

    /// Builds ChatInfo from the pane and the transcript summary; broadcasts it when it changed.
    fn publish_info(&self) {
        let found = registry().find_pane(&self.host_id, &self.pane_id);
        let msg = {
            let mut st = self.st.borrow_mut();
            let queue = current_queue(&st);
            {
                let State { ledger, items, .. } = &mut *st;
                ledger.match_items(items, &queue);
            }
            let agent = found.as_ref().and_then(|f| f.pane.agent.as_ref());
            let m = &st.meta;
            let title =
                m.custom_title.clone().filter(|t| !t.is_empty()).or_else(|| m.title.clone().filter(|t| !t.is_empty())).or_else(|| found.as_ref().map(|f| pane_title(&f.pane)));
            let mut stats = m.stats.clone();
            if stats.started_at.is_none() {
                stats.started_at = agent.and_then(|a| a.started_at).filter(|s| *s != 0).and_then(|s| date::to_iso(s as f64));
            }
            let info = ChatInfo {
                host_id: self.host_id.clone(),
                pane_id: self.pane_id.clone(),
                agent: agent.map(|a| a.kind).or(st.agent),
                running: agent.is_some(),
                transcript_path: agent.and_then(|a| a.transcript_path.clone()).or_else(|| st.transcript.clone()),
                session_id: agent.and_then(|a| a.session_id.clone()).or_else(|| st.info.session_id.clone()),
                mapped_by: agent.and_then(|a| a.mapped_by).or(st.info.mapped_by),
                cwd: m.cwd.clone().or_else(|| found.as_ref().map(|f| f.pane.cwd.clone())),
                title,
                model: m.model.clone(),
                status: m.status.clone(),
                git_branch: m.git_branch.clone(),
                version: m.version.clone(),
                mode: st.screen.mode.clone().or_else(|| agent.and_then(|a| a.mode.clone())).or_else(|| m.mode.clone()),
                ultracode: st.screen.ultracode.or(agent.and_then(|a| a.ultracode)),
                goal: if st.goal_unsure { None } else { m.goal.clone() },
                rc: m.rc.clone().or_else(|| st.remote.clone()),
                share: m.share.clone(),
                queue,
                held: Some(super::held::list(&self.host_id, &self.pane_id)).filter(|h| !h.is_empty()),
                chosen: super::chosen::get(&self.host_id, &self.pane_id),
                turn: st.live.turn.clone().or_else(|| m.turn.clone()),
                sends: Some(st.ledger.list()),
                tasks: last40(m.tasks.values()),
                subagents: last40(merged_agents(m, &st.live).iter()),
                artifacts: last40(m.artifacts.values()),
                stats,
                error: found.is_none().then(|| "This pane no longer exists".to_owned()),
            };
            if to_json(&info) == to_json(&st.info) {
                return;
            }
            st.info = info.clone();
            (st.ready && st.behind.is_none()).then(|| ServerMsg::ChatInfo { key: self.key.clone(), info: Box::new(info) })
        };
        if let Some(m) = msg {
            self.broadcast(&m);
        }
    }

    /// What OpenCode's own record says about the conversation as a whole: title, cost, lines
    /// changed, share link.
    fn apply_open_code_info(&self, info: Option<OcdbSessionInfo>) {
        let Some(info) = info else { return };
        let mut st = self.st.borrow_mut();
        let m = &mut st.meta;
        // the title OpenCode gives a conversation it hasn't named yet says nothing
        if let Some(t) = info.title.filter(|t| !t.is_empty() && !t.starts_with("New session - ")) {
            m.title = Some(t);
        }
        if let Some(c) = info.cost {
            m.stats.cost_usd = Some(c.into());
        }
        if let Some(a) = info.additions {
            m.stats.lines_added = Some(a.into());
        }
        if let Some(d) = info.deletions {
            m.stats.lines_removed = Some(d.into());
        }
        m.share = info.share.filter(|s| !s.is_empty());
        let cost = m.stats.cost_usd;
        if let Some(s) = m.status.as_mut() {
            s.cost_usd = cost;
        }
    }

    fn publish_snapshot(&self, items: Vec<ChatItem>) {
        let first = {
            let mut st = self.st.borrow_mut();
            // a very long conversation keeps only its latest messages, like the ones read line by line
            st.items = if items.len() > KEEP { items[items.len() - KEEP..].to_vec() } else { items };
            // the whole conversation, read again: its goal is current
            st.goal_unsure = false;
            !st.ready
        };
        self.cache_later();
        self.st.borrow_mut().ready = true;
        self.publish_info();
        let msg = {
            let st = self.st.borrow();
            (first || !st.subs.is_empty()).then(|| reset_msg(&self.key, &st))
        };
        if let Some(m) = msg {
            self.broadcast(&m);
        }
        if self.settle_screen_live() {
            self.live_soon();
        }
    }

    /// Formats rewritten per step (Gemini's session file, Antigravity's transcript): rebuild on change.
    fn follow_doc(&self, path: &str, gen: u64, doc: impl Doc + 'static) {
        let Ok(host) = self.host() else { return };
        // a new conversation has no file until the first message; show the empty chat meanwhile
        let _ = self.later(2500, move |w| {
            let due = {
                let st = w.st.borrow();
                gen == st.generation && !st.ready && st.snapshot_timer.is_none()
            };
            if due {
                w.publish_snapshot(vec![]);
            }
        });
        let doc = Rc::new(RefCell::new(doc));
        let me = self.me.clone();
        let stream = host.stream(
            &format!("exec tail -c +1 -F {} 2>/dev/null", shq(path)),
            move |line| {
                let Some(w) = me.upgrade() else { return };
                if gen != w.st.borrow().generation {
                    return;
                }
                let Some(rec) = json::parse(&line) else { return };
                doc.borrow_mut().apply(&rec);
                if w.st.borrow().snapshot_timer.is_some() {
                    return;
                }
                let doc = doc.clone();
                let ms = if w.st.borrow().ready { 350 } else { 600 };
                let t = w.later(ms, move |w| {
                    let items = {
                        let mut st = w.st.borrow_mut();
                        st.snapshot_timer = None;
                        if gen != st.generation {
                            return;
                        }
                        doc.borrow().items(&mut st.meta)
                    };
                    w.publish_snapshot(items);
                });
                w.st.borrow_mut().snapshot_timer = Some(t);
            },
            false,
        );
        self.st.borrow_mut().tail = stream.ok();
        self.watch_tail(gen);
    }

    fn follow_open_code(&self, uri: &str, gen: u64) {
        let ep = parse_endpoint(uri).filter(|e| !e.session.is_empty());
        let (Some(ep), Ok(host)) = (ep, self.host()) else {
            self.st.borrow_mut().ready = true;
            return;
        };
        let session = ep.session.clone();
        let cwd = registry().find_pane(&self.host_id, &self.pane_id).map(|f| f.pane.cwd);
        let Ok(client) = OpenCodeClient::new(&host, uri, cwd.as_deref()) else {
            self.st.borrow_mut().ready = true;
            return;
        };
        let client = Rc::new(client);
        let base = ep.base.clone();
        let refresh: Rc<dyn Fn(Rc<ChatWatcher>) -> LocalBoxFuture<'static, ()>> = {
            let client = client.clone();
            Rc::new(move |w: Rc<ChatWatcher>| {
                let (client, session, base) = (client.clone(), session.clone(), base.clone());
                Box::pin(async move { w.refresh_open_code(&client, &session, &base, gen).await })
            })
        };
        let later: Later = {
            let refresh = refresh.clone();
            Rc::new(move |w: &Rc<ChatWatcher>| {
                if w.st.borrow().snapshot_timer.is_some() {
                    return;
                }
                let refresh = refresh.clone();
                let t = w.later(300, move |w| {
                    w.st.borrow_mut().snapshot_timer = None;
                    let w = w.clone();
                    spawn(async move { refresh(w).await });
                });
                w.st.borrow_mut().snapshot_timer = Some(t);
            })
        };
        {
            let w = self.rc();
            let refresh = refresh.clone();
            spawn(async move {
                w.st.borrow_mut().snapshot_timer = None;
                refresh(w).await
            });
        }
        // the event stream tells us when anything changes
        let me = self.me.clone();
        let l2 = later.clone();
        let stream = host.stream(
            &format!("exec curl -sN {}", shq(&format!("{}/event", ep.base))),
            move |line| {
                let Some(w) = me.upgrade() else { return };
                if gen == w.st.borrow().generation && line.starts_with("data:") {
                    l2(&w);
                }
            },
            false,
        );
        self.st.borrow_mut().tail = stream.ok();
        self.watch_tail(gen);
        self.open_code_poll(gen, later);
    }

    /// `setInterval(…, 5000)` until the conversation changes or the watcher stops.
    fn open_code_poll(&self, gen: u64, later: Later) {
        let t = self.later(5000, move |w| {
            let go = {
                let st = w.st.borrow();
                gen == st.generation && !st.stopped
            };
            if go {
                later(w);
                w.open_code_poll(gen, later);
            }
        });
        let mut st = self.st.borrow_mut();
        rt::clear(&mut st.opencode_poll);
        st.opencode_poll = Some(t);
    }

    async fn refresh_open_code(&self, client: &Rc<OpenCodeClient>, session: &str, base: &str, gen: u64) {
        let (msgs, info) = join(client.messages(), client.info()).await;
        let msgs = msgs.ok().flatten();
        let info = info.ok().flatten();
        if gen != self.st.borrow().generation {
            return;
        }
        let Some(Value::Array(msgs)) = msgs else { return };
        let items = opencode_items(&msgs, &mut self.st.borrow_mut().meta);
        let info = info.map(|i| {
            let i = Some(&i);
            OcdbSessionInfo {
                title: i.at("title").str().map(str::to_owned),
                cost: i.at("cost").num(),
                additions: i.at("summary").at("additions").num(),
                deletions: i.at("summary").at("deletions").num(),
                files: None,
                share: i.at("share").at("url").str().map(str::to_owned),
            }
        });
        self.apply_open_code_info(info);
        self.publish_snapshot(items);
        let (perms, questions) = join(client.permissions(), client.questions()).await;
        if gen != self.st.borrow().generation {
            return;
        }
        let perms = perms.ok().flatten();
        let questions = questions.ok().flatten();
        let mine = perms.as_ref().and_then(Value::as_array).and_then(|a| a.iter().find(|p| Some(*p).at("sessionID").str() == Some(session)).cloned());
        let next = mine.map(|m| {
            let m = Some(&m);
            let mut detail: Vec<String> = vec![];
            if let Some(t) = m.at("type").filter(|v| Some(*v).truthy()) {
                detail.push(json::value_string(t));
            }
            for p in m.at("patterns").arr().unwrap_or(&vec![]) {
                if Some(p).truthy() {
                    detail.push(json::value_string(p));
                }
            }
            ApiPermission {
                id: m.at("id").map(json::value_string).unwrap_or_default(),
                base: base.to_owned(),
                title: m.at("title").filter(|v| !v.is_null()).or(m.at("permission").filter(|v| !v.is_null())).map_or_else(|| "Permission requested".to_owned(), json::value_string),
                detail: Some(detail.join(" ")),
            }
        });
        let mut changed = false;
        {
            let mut st = self.st.borrow_mut();
            if to_json(&next) != to_json(&st.api_permission) {
                st.api_permission = next;
                changed = true;
            }
            let asked = questions.as_ref().and_then(Value::as_array).and_then(|a| {
                a.iter().find(|q| {
                    let q = Some(*q);
                    q.at("sessionID").str() == Some(session) && q.at("questions").arr().is_some_and(|x| !x.is_empty())
                })
            });
            let asked_id = asked.and_then(|q| Some(q).at("id").str().map(str::to_owned));
            if asked_id.as_deref() != st.api_question.as_ref().map(|q| q.flow.id()) {
                st.api_question = asked.and_then(|q| {
                    let qs: Vec<OpenCodeQuestion> = serde_json::from_value(q.get("questions")?.clone()).ok()?;
                    let flow = OpenCodeQuestionFlow::new(asked_id.clone().unwrap_or_default(), qs).ok()?;
                    Some(ApiQuestion { flow, client: client.clone() })
                });
                changed = true;
            }
        }
        if changed {
            self.kick_screen();
        }
    }

    /// OpenCode without a server (plain `opencode`): read its database. A cheap change stamp is
    /// checked about every second while it works (every few seconds when idle), and only rows
    /// written since the last read are fetched.
    fn follow_open_code_db(&self, uri: &str, gen: u64) {
        let (Some(session), Ok(host)) = (ocdb_session(uri).map(str::to_owned), self.host()) else {
            return self.publish_snapshot(vec![]);
        };
        let db = Rc::new(RefCell::new((OcdbCache::new(), -1.0f64, now_ms())));
        let w = self.rc();
        spawn(async move { w.ocdb_tick(host, session, gen, db).await });
    }

    fn ocdb_tick(&self, host: Host, session: String, gen: u64, db: Rc<RefCell<(OcdbCache, f64, u64)>>) -> LocalBoxFuture<'static, ()> {
        let w = self.rc();
        Box::pin(async move {
            {
                let st = w.st.borrow();
                if gen != st.generation || st.stopped {
                    return;
                }
            }
            let step = async {
                let stamp = ocdb_stamp_of(&host, &session).await?;
                if gen != w.st.borrow().generation {
                    return Ok(false);
                }
                let (last, ready) = (db.borrow().1, w.st.borrow().ready);
                if stamp != last || !ready {
                    {
                        let mut d = db.borrow_mut();
                        d.1 = stamp;
                        d.2 = now_ms();
                    }
                    // a little overlap: rows written in the same millisecond as the last read
                    let since = (db.borrow().0.stamp - 1500.0).max(0.0);
                    let (rows, shape, info) =
                        futures_util::future::join3(ocdb_rows_of(&host, &session, since), ocdb_shape_of(&host, &session), ocdb_session_info_of(&host, &session)).await;
                    let ((messages, parts), shape, info) = (rows?, shape?, info?);
                    if gen != w.st.borrow().generation {
                        return Ok(false);
                    }
                    let list = {
                        let mut d = db.borrow_mut();
                        d.0.merge(&messages, &parts);
                        d.0.retain(&shape.ids);
                        d.0.list(shape.revert.as_deref())
                    };
                    let items = opencode_items(&list, &mut w.st.borrow_mut().meta);
                    w.apply_open_code_info(info);
                    w.publish_snapshot(items);
                }
                Ok::<bool, HubError>(true)
            };
            match step.await {
                Ok(true) => {}
                Ok(false) => return,
                Err(_) => {
                    if !w.st.borrow().ready {
                        w.publish_snapshot(vec![]);
                    }
                }
            }
            let busy = w.st.borrow().screen.working || now_ms().saturating_sub(db.borrow().2) < 15_000;
            let _ = w.later(if busy { 900 } else { 2500 }, move |w| {
                let f = w.ocdb_tick(host, session, gen, db);
                spawn(f);
            });
        })
    }

    /// Answers a permission request, or a question from the agent, through the agent's API (OpenCode).
    pub async fn answer_api(&self, option: u32, text: Option<&str>) -> HubResult<bool> {
        let p = self.st.borrow().api_permission.clone();
        if let Some(p) = p {
            let reply = match option {
                1 => "once",
                2 => "always",
                _ => "reject",
            };
            let ok = opencode_post(&self.host()?, &format!("{}/permission/{}/reply", p.base, p.id), &serde_json::json!({ "reply": reply })).await;
            if !ok {
                return Err(HubError::plain("OpenCode did not accept the permission response. The question is still pending."));
            }
            self.st.borrow_mut().api_permission = None;
            self.kick_screen();
            return Ok(ok);
        }
        let answered = {
            let mut st = self.st.borrow_mut();
            let Some(q) = st.api_question.as_mut() else { return Ok(false) };
            if q.flow.done() {
                return Ok(false);
            }
            q.flow.answer(option, text).map_err(HubError::plain)?
        };
        if !answered {
            self.kick_screen();
            return Ok(true);
        }
        // all answered: send them; if OpenCode won't take them, the next refresh asks afresh
        let Some(q) = self.st.borrow_mut().api_question.take() else { return Ok(false) };
        if q.client.reply_question(q.flow.id(), q.flow.answers()).await.is_err() {
            return Err(HubError::plain("OpenCode did not take the answers. The question is still open; try again."));
        }
        self.kick_screen();
        Ok(true)
    }

    /// Notices when the current tail stops without being asked to (an SSH or paired link dropped).
    fn watch_tail(&self, gen: u64) {
        let Some(t) = self.st.borrow().tail.clone() else { return };
        let me = self.me.clone();
        spawn(async move {
            t.exited.wait().await;
            let Some(w) = me.upgrade() else { return };
            let retries = {
                let mut st = w.st.borrow_mut();
                if gen != st.generation || !st.tail.as_ref().is_some_and(|x| x.same(&t)) || st.stopped {
                    return;
                }
                st.tail_lost = true;
                st.retries += 1;
                st.retries - 1
            };
            // try again soon rather than at the next poll; backing off if it keeps ending at once
            let ms = retry_ms(retries);
            let timer = w.later(ms, |w| {
                if !w.st.borrow().stopped {
                    w.sync();
                }
            });
            let mut st = w.st.borrow_mut();
            rt::clear(&mut st.retry_timer);
            st.retry_timer = Some(timer);
        });
    }

    /// Clears what was read, for a file read from its start.
    fn restart(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.items = vec![];
            st.meta = new_meta();
            st.pending = vec![];
            st.ready = false;
            st.behind = None;
            st.reader = None;
            st.older_from = None;
            st.line_at.clear();
            st.tail_start = false;
            st.replay = None;
            st.goal_unsure = false;
            st.held_history = None;
        }
        self.stop_live();
    }

    /// Reads the summary of the part before the last stretch, then replays the lines read since.
    fn summary_before(&self, path: &str, from: u64, prefix: TranscriptMeta, saved: u64, gen: u64) {
        let id = {
            let mut st = self.st.borrow_mut();
            st.replays += 1;
            let id = st.replays;
            st.replay = Some((id, vec![]));
            id
        };
        let w = self.rc();
        let path = path.to_owned();
        spawn(async move {
            let prefix = Rc::new(RefCell::new(prefix));
            w.read_meta(&path, from, prefix.clone(), saved).await;
            let (codex, agent) = w.kind();
            {
                let mut st = w.st.borrow_mut();
                if gen != st.generation || st.stopped || st.replay.as_ref().map(|r| r.0) != Some(id) {
                    return;
                }
                let (_, replay) = st.replay.take().expect("the replay");
                let mut meta = prefix.borrow().clone();
                for e in &replay {
                    Self::normalize_entry(codex, agent, e, &mut meta);
                }
                st.meta = meta;
            }
            w.settle_goal();
            let (ready, behind) = {
                let st = w.st.borrow();
                (st.ready, st.behind.is_some())
            };
            if ready && !behind {
                w.publish_info();
                w.save();
            } else {
                w.cache_later();
            }
        });
    }

    async fn follow(&self, path: String) {
        let doc = path.starts_with("opencode://") || is_ocdb(Some(&path)) || path.contains("/.gemini/tmp/") || path.contains("/.gemini/antigravity-cli/brain/");
        let proc = proc_of(registry().pane_agent(&self.host_id, &self.pane_id).as_ref());
        // the same file again after a dropped connection: carry on from the byte where it stopped
        let (mut again, gen) = {
            let mut st = self.st.borrow_mut();
            let again = !doc && st.transcript.as_deref() == Some(path.as_str()) && st.ready && st.reader.is_some();
            if let Some(t) = st.tail.take() {
                t.kill();
            }
            st.tail_lost = false;
            st.transcript = Some(path.clone());
            st.proc = proc;
            st.generation += 1;
            (again, st.generation)
        };
        if !again {
            self.restart();
        }
        if path.starts_with("opencode://") {
            return self.follow_open_code(&path, gen);
        }
        if is_ocdb(Some(&path)) {
            return self.follow_open_code_db(&path, gen);
        }
        if path.contains("/.gemini/tmp/") {
            return self.follow_doc(&path, gen, GeminiDoc::new());
        }
        if path.contains("/.gemini/antigravity-cli/brain/") {
            return self.follow_doc(&path, gen, AntigravityDoc::new());
        }
        let t0 = now_ms();
        if debug() {
            console::log(&format!("[follow] {} {} {path}", if again { "resume" } else { "start" }, self.key));
        }
        let Ok(host) = self.host() else { return };
        let offset = if again { self.st.borrow().reader.as_ref().map_or(0, |r| r.offset) } else { 0 };
        let Ok(r) = host.run(&size_command(&path, offset)).await else { return };
        let size = parse_size(&r.stdout);
        let (size, aligned) = (size.size, size.aligned);
        if self.stale(gen) {
            return;
        }
        // shorter than what was read, or no line starts where it stopped: the file was rewritten (or
        // the count is off), so read it from the start rather than mid-line
        if again && (size < offset || !aligned) {
            again = false;
            self.restart();
        }
        // away a long while (a saved copy from hours ago): rather than read everything said since,
        // keep the saved chat on screen and read only the last stretch, while the summary catches up
        // from the lines in between
        let (tail_first, agent) = {
            let st = self.st.borrow();
            (st.agent != Some(AgentKind::Copilot) && st.older_from.is_some(), st.agent)
        };
        let jump = again && tail_first && size - offset > TAIL_BYTES;
        if jump {
            let from = size - TAIL_BYTES;
            {
                let mut st = self.st.borrow_mut();
                st.behind = Some(reset_msg(&self.key, &st));
            }
            self.publish_info();
            let prefix = load_meta(&save_meta(&self.st.borrow().meta)).unwrap_or_else(new_meta);
            self.summary_before(&path, from, prefix, offset, gen);
            let mut st = self.st.borrow_mut();
            st.items = vec![];
            st.pending = vec![];
            st.line_at.clear();
            // above anything shown before, so nothing the app still holds is mistaken for these
            st.seq = st.seq.max(TAIL_SEQ) + TAIL_SEQ;
            st.reader = Some(TranscriptReader::new(from));
            st.older_from = Some(size);
            st.tail_start = true;
        }
        if !again {
            let codex = agent == Some(AgentKind::Codex) || re!(r"/\.codex/sessions/").is_match(&path);
            {
                let mut st = self.st.borrow_mut();
                st.seq = 0;
                st.reader = Some(TranscriptReader::new(0));
                st.codex = codex;
            }
            // a Codex segment file starts after a rewind: the history it continues comes first
            let history = if codex { codex_history(&host, &path, 0).await.unwrap_or_default() } else { vec![] };
            if self.stale(gen) {
                return;
            }
            if agent != Some(AgentKind::Copilot) {
                self.st.borrow_mut().older_from = Some(0);
                if size > TAIL_BYTES {
                    // a long conversation: only its last stretch, shown as soon as it's in, while what's
                    // known about it as a whole (title, goal, tasks) is read alongside
                    let from = size - TAIL_BYTES;
                    self.summary_before(&path, from, new_meta(), 0, gen);
                    let mut st = self.st.borrow_mut();
                    st.seq = TAIL_SEQ;
                    st.reader = Some(TranscriptReader::new(from));
                    st.older_from = Some(size);
                    st.tail_start = true;
                    // the earlier segments' lines are read in once scrolling back reaches this file's start
                    if !history.is_empty() {
                        st.held_history = Some(history.clone());
                    }
                }
            }
            if codex && self.st.borrow().held_history.is_none() {
                for line in &history {
                    let Some(e) = json::parse(line) else { continue };
                    {
                        let mut st = self.st.borrow_mut();
                        let State { meta, items, seq, .. } = &mut *st;
                        for d in normalize_codex(&e, meta) {
                            items.push(with_seq(d, *seq));
                            *seq += 1;
                        }
                    }
                    self.trim();
                }
            }
        }
        // Codex: join the thread's live events too, for the reply as it's written and the turn under way
        let (codex, has_live) = {
            let st = self.st.borrow();
            (st.codex, st.live_tail.is_some())
        };
        if codex && !has_live {
            self.start_live(&path, gen);
        }
        let start = self.st.borrow().reader.as_ref().map_or(0, |r| r.offset);
        let total = size.saturating_sub(start);
        let remaining = size as i64 - start as i64;
        // nothing written since the saved copy: its goal stands
        if remaining <= 0 {
            self.settle_goal();
        }
        // a lot was said meanwhile: read it out of sight, keeping the chat as it was until it's all in
        if again && remaining > CATCHUP_BYTES as i64 && !jump {
            self.publish_info();
            let mut st = self.st.borrow_mut();
            st.behind = Some(reset_msg(&self.key, &st));
        }
        let init = Rc::new(Initial { gen, total, remaining: Cell::new(remaining), filling: Cell::new(false), quiet: RefCell::new(None), t0 });
        // If the file is empty or tail never catches up, still show what we have once it goes quiet.
        if self.reading() {
            let i2 = init.clone();
            *init.quiet.borrow_mut() = Some(self.later(if remaining > 0 { FIRST_LINE_MS } else { 0 }, move |w| w.finish_initial(&i2)));
        }
        if self.reading() && remaining > 0 {
            self.report_progress(0, total, true);
        }
        // a remote machine leaves out image data, which is most of a long session's file; its tail
        // ends when stdin closes, since closing the connection alone doesn't stop it there
        let remote = self.on_remote_host();
        let me = self.me.clone();
        let i2 = init.clone();
        let stream = host.stream(
            &tail_command(&path, start, remote, true),
            move |line| {
                if let Some(w) = me.upgrade() {
                    w.tail_line(&line, &i2);
                }
            },
            remote,
        );
        self.st.borrow_mut().tail = stream.ok();
        self.watch_tail(gen);
    }

    /// One line of the transcript's tail.
    fn tail_line(&self, line: &str, init: &Rc<Initial>) {
        let gen = init.gen;
        let (r, at, codex, agent) = {
            let mut st = self.st.borrow_mut();
            if gen != st.generation {
                return;
            }
            let Some(reader) = st.reader.as_mut() else { return };
            let Some(r) = reader.line(line) else { return };
            let at = reader.offset - r.bytes;
            st.retries = 0;
            (r, at, st.codex, st.agent)
        };
        init.remaining.set(init.remaining.get() - r.bytes as i64);
        // once this line's entry is in the summary, not before: it may be the goal being met
        if init.remaining.get() <= 0 {
            let me = self.me.clone();
            spawn(async move {
                yield_now().await;
                if let Some(w) = me.upgrade() {
                    if gen == w.st.borrow().generation {
                        w.settle_goal();
                    }
                }
            });
        }
        if let Some(e) = r.entry {
            let items = {
                let mut st = self.st.borrow_mut();
                if st.tail_start {
                    st.older_from = Some(at);
                    st.tail_start = false;
                }
                if let Some((_, replay)) = st.replay.as_mut() {
                    replay.push(e.clone());
                }
                let State { meta, seq, .. } = &mut *st;
                let items: Vec<ChatItem> = Self::normalize_entry(codex, agent, &e, meta)
                    .into_iter()
                    .map(|d| {
                        let it = with_seq(d, *seq);
                        *seq += 1;
                        it
                    })
                    .collect();
                if st.older_from.is_some() {
                    for it in &items {
                        st.line_at.insert(it.seq(), at);
                    }
                }
                items
            };
            if !items.is_empty() {
                self.st.borrow_mut().items.extend(items.iter().cloned());
                self.trim();
                if !self.reading() {
                    self.queue(items.clone());
                }
                // the record of something that was being written live: the live copy can go
                if codex {
                    let ids: Vec<Option<String>> = items.iter().flat_map(record_ids).collect();
                    let refs: Vec<Option<&str>> = ids.iter().map(Option::as_deref).collect();
                    if self.st.borrow_mut().live.settle(&refs) {
                        self.live_soon();
                    }
                }
                if self.settle_screen_live() {
                    self.live_soon();
                }
            }
        }
        if self.reading() {
            rt::clear(&mut init.quiet.borrow_mut());
            if init.remaining.get() <= 0 {
                self.finish_initial(init);
            } else {
                let i2 = init.clone();
                *init.quiet.borrow_mut() = Some(self.later(QUIET_MS, move |w| w.finish_initial(&i2)));
                self.report_progress(init.total.saturating_sub(init.remaining.get().max(0) as u64), init.total, false);
            }
        } else {
            self.schedule_info();
        }
    }

    fn reading(&self) -> bool {
        let st = self.st.borrow();
        !st.ready || st.behind.is_some()
    }

    fn stale(&self, gen: u64) -> bool {
        let st = self.st.borrow();
        gen != st.generation || st.stopped
    }

    fn finish_initial(&self, init: &Rc<Initial>) {
        rt::clear(&mut init.quiet.borrow_mut());
        if !self.reading() || init.gen != self.st.borrow().generation {
            return;
        }
        // short of a page: with nothing at all to show (the stretch was one screenshot), read back
        // first; otherwise show what's there now and fill the page in from before it
        let (short, empty) = {
            let st = self.st.borrow();
            (st.seq >= TAIL_SEQ && st.items.len() < PAGE && older_in_file(&st) && !init.filling.get(), st.items.is_empty())
        };
        if short && empty {
            init.filling.set(true);
            let (w, i2) = (self.rc(), init.clone());
            let older = self.load_older();
            spawn(async move {
                older.await;
                w.finish_initial(&i2);
            });
            return;
        }
        if short {
            init.filling.set(true);
            let (w, gen) = (self.rc(), init.gen);
            let older = self.load_older();
            spawn(async move {
                older.await;
                let msg = {
                    let st = w.st.borrow();
                    (gen == st.generation && st.ready && st.behind.is_none()).then(|| reset_msg(&w.key, &st))
                };
                if let Some(m) = msg {
                    w.broadcast(&m);
                }
                w.save();
            });
        }
        if debug() {
            console::log(&format!("[follow] ready {} {} ms {} items", self.key, now_ms().saturating_sub(init.t0), self.st.borrow().items.len()));
        }
        {
            let mut st = self.st.borrow_mut();
            st.ready = true;
            st.behind = None;
            st.progress = None;
        }
        self.publish_info();
        let msg = reset_msg(&self.key, &self.st.borrow());
        self.broadcast(&msg);
        // saved at once, not 10 s later: an app closed before then would read it all again next time
        self.save();
    }

    // ---------- Codex live events (the reply as it's written) ----------

    /// Joins the thread on Codex's app-server through a helper on the machine (codex_live.rs). The
    /// helper needs node and the daemon's socket; without them it ends at once, and is tried again
    /// later rather than at once.
    fn start_live(&self, path: &str, gen: u64) {
        self.stop_live();
        let id = codex_thread_id(path);
        // only a real rollout under Codex's own folder names a thread its daemon has
        if !re!(r"/\.codex/sessions/").is_match(path) || !re!(r"^[0-9a-f-]{36}$").is_match(&id) {
            return;
        }
        let Ok(host) = self.host() else { return };
        let me = self.me.clone();
        let mine: Rc<RefCell<Option<StreamHandle>>> = Rc::default();
        let m2 = mine.clone();
        let Ok(t) = host.stream(
            &codex_live_command(&id),
            move |line| {
                let Some(w) = me.upgrade() else { return };
                let current = {
                    let st = w.st.borrow();
                    gen == st.generation && matches!((&st.live_tail, &*m2.borrow()), (Some(a), Some(b)) if a.same(b))
                };
                if current {
                    w.on_live_line(&line);
                }
            },
            true,
        ) else {
            return;
        };
        *mine.borrow_mut() = Some(t.clone());
        self.st.borrow_mut().live_tail = Some(t.clone());
        let me = self.me.clone();
        let path = path.to_owned();
        spawn(async move {
            let code = t.exited.wait().await;
            let Some(w) = me.upgrade() else { return };
            let wait = {
                let mut st = w.st.borrow_mut();
                if !st.live_tail.as_ref().is_some_and(|x| x.same(&t)) || st.stopped || gen != st.generation {
                    return;
                }
                st.live_tail = None;
                // no node on the machine (127), or nothing listening: not worth asking every few seconds
                if code == 127 || st.live_retries >= 8 {
                    10 * 60_000
                } else {
                    st.live_retries += 1;
                    60_000u64.min(2000 * 2u64.pow(st.live_retries - 1))
                }
            };
            let timer = w.later(wait, move |w| {
                let go = {
                    let st = w.st.borrow();
                    gen == st.generation && !st.stopped && st.transcript.as_deref() == Some(path.as_str())
                };
                if go {
                    w.start_live(&path, gen);
                }
            });
            let mut st = w.st.borrow_mut();
            rt::clear(&mut st.live_retry_timer);
            st.live_retry_timer = Some(timer);
        });
    }

    /// The reply being written on screen, carried on from the last read when its start has
    /// scrolled away, without what a record holds already. True when it changed.
    fn read_screen_live(&self, lines: &[String], width: Option<u32>, kind: AgentKind, working: bool) -> bool {
        // the turn is over: what was read waits for its record, so the reply doesn't blink out
        // between the two, and goes after a while if no record has it
        if !working {
            if self.st.borrow().screen_live.is_some() && self.st.borrow().screen_live_expiry.is_none() {
                let t = self.later(SCREEN_LIVE_WAIT_MS, |w| {
                    let had = {
                        let mut st = w.st.borrow_mut();
                        st.screen_live_expiry = None;
                        st.screen_live.take().is_some()
                    };
                    if had {
                        w.live_soon();
                    }
                });
                self.st.borrow_mut().screen_live_expiry = Some(t);
            }
            return false;
        }
        let mut st = self.st.borrow_mut();
        rt::clear(&mut st.screen_live_expiry);
        let read = screen_reply(lines, kind, width);
        let before = st.screen_live.take();
        let next = read.and_then(|r| {
            let text = match &before {
                Some(b) if !r.whole => carry_on(&b.text, &r.text),
                _ => r.text,
            };
            let text = unrecorded(&text, &recent_replies(&st.items));
            let id = before.as_ref().map_or_else(|| format!("screen:{}", now_ms()), |b| b.id.clone());
            (!text.is_empty()).then_some(LiveItem { id, kind: LiveKind::Text, text, turn_id: None })
        });
        let changed = next.as_ref().map(|n| &n.text) != before.as_ref().map(|b| &b.text);
        st.screen_live = next;
        changed
    }

    /// Records came in: what they hold leaves the live text. The app drops the screen's copy when
    /// a new reply is on record, which can be an earlier turn's, so what's left is sent again.
    /// True when there's something to send.
    fn settle_screen_live(&self) -> bool {
        let mut st = self.st.borrow_mut();
        let Some(live) = st.screen_live.take() else { return false };
        let text = unrecorded(&live.text, &recent_replies(&st.items));
        if !text.is_empty() {
            st.live_json = String::new();
        }
        st.screen_live = (!text.is_empty()).then_some(LiveItem { text, ..live });
        true
    }

    fn stop_live(&self) {
        let (t, had, stopped) = {
            let mut st = self.st.borrow_mut();
            rt::clear(&mut st.live_retry_timer);
            rt::clear(&mut st.live_timer);
            rt::clear(&mut st.live_expiry);
            rt::clear(&mut st.screen_live_expiry);
            let t = st.live_tail.take();
            let had = !st.live.items.is_empty() || st.screen_live.is_some();
            st.live.clear();
            st.screen_live = None;
            st.live_json = String::new();
            (t, had, st.stopped)
        };
        if let Some(t) = t {
            t.kill();
        }
        if had && !stopped {
            self.broadcast(&ServerMsg::ChatLive { key: self.key.clone(), items: vec![] });
        }
    }

    fn on_live_line(&self, line: &str) {
        let ch = {
            let mut st = self.st.borrow_mut();
            let ch = st.live.apply(line);
            if line.starts_with(r#"{"e":"resumed""#) {
                st.live_retries = 0;
            }
            ch
        };
        let yes = |b: Option<bool>| b == Some(true);
        if yes(ch.live) {
            self.live_soon();
            self.arm_live_expiry();
        }
        if yes(ch.turn) || yes(ch.agents) {
            self.schedule_info();
        }
        // the screen shows the change a moment later; read it now rather than at the next poll
        if yes(ch.turn) || yes(ch.status) || yes(ch.queue) || yes(ch.ask) {
            self.kick_screen();
        }
        if yes(ch.turn) || yes(ch.status) || ch.name.is_some() {
            registry().refresh(&self.host_id, 150);
        }
        if yes(ch.usage) {
            // the numbers are in the rollout by now; the 8 s cache would show the old ones meanwhile
            registry().forget_status(&self.host_id);
            registry().refresh(&self.host_id, 400);
        }
    }

    /// Sends the live text to everyone watching, a few times a second at most.
    fn live_soon(&self) {
        if self.st.borrow().live_timer.is_some() {
            return;
        }
        let t = self.later(80, |w| {
            let msg = {
                let mut st = w.st.borrow_mut();
                st.live_timer = None;
                let items = live_items(&st);
                let json = to_json(&items);
                if json == st.live_json {
                    return;
                }
                st.live_json = json;
                ServerMsg::ChatLive { key: w.key.clone(), items }
            };
            w.broadcast(&msg);
        });
        self.st.borrow_mut().live_timer = Some(t);
    }

    /// A finished item whose record never shows up (ids that don't match) is dropped after a while.
    fn arm_live_expiry(&self) {
        if self.st.borrow().live_expiry.is_some() {
            return;
        }
        let t = self.later(1500, |w| {
            let (expired, more) = {
                let mut st = w.st.borrow_mut();
                st.live_expiry = None;
                let expired = st.live.expire(6000.0, now_ms() as f64);
                (expired, !st.live.done.is_empty())
            };
            if expired {
                w.live_soon();
            }
            if more {
                w.arm_live_expiry();
            }
        });
        self.st.borrow_mut().live_expiry = Some(t);
    }

    pub fn schedule_info(&self) {
        if self.st.borrow().info_timer.is_some() {
            return;
        }
        let t = self.later(150, |w| {
            w.st.borrow_mut().info_timer = None;
            w.publish_info();
        });
        self.st.borrow_mut().info_timer = Some(t);
    }

    fn queue(&self, items: Vec<ChatItem>) {
        let mut st = self.st.borrow_mut();
        st.pending.extend(items);
        if st.flush_timer.is_none() {
            st.flush_timer = Some(self.later(60, |w| {
                let batch = {
                    let mut st = w.st.borrow_mut();
                    st.flush_timer = None;
                    std::mem::take(&mut st.pending)
                };
                if !batch.is_empty() {
                    w.broadcast(&ServerMsg::ChatAppend { key: w.key.clone(), items: batch });
                }
            }));
        }
    }

    /// The pane printed something: re-read the screen soon (coalescing bursts of output).
    pub fn screen_changed(&self) {
        if self.st.borrow().change_timer.is_some() {
            return;
        }
        let t = self.later(if hidden() { QUIET_SCREEN_MS } else { 120 }, |w| {
            w.st.borrow_mut().change_timer = None;
            w.kick_screen();
        });
        self.st.borrow_mut().change_timer = Some(t);
    }

    pub async fn poll_screen(&self) {
        {
            let mut st = self.st.borrow_mut();
            if st.stopped {
                return;
            }
            rt::clear(&mut st.screen_timer);
        }
        let _ = self.read_screen().await;
        self.next_screen();
    }

    async fn read_screen(&self) -> HubResult<()> {
        let host = self.host()?;
        let caps = capture_panes(&host, std::slice::from_ref(&self.pane_id)).await?;
        let lines = caps.get(&self.pane_id);
        let pane = registry().find_pane(&self.host_id, &self.pane_id).map(|f| f.pane);
        let (Some(lines), Some(pane), true) = (lines, &pane, pane.as_ref().is_some_and(|p| p.agent.is_some())) else {
            let msg = {
                let mut st = self.st.borrow_mut();
                if st.screen_json == "none" {
                    return Ok(());
                }
                st.screen_json = "none".into();
                st.screen = blank_screen();
                ServerMsg::ChatScreen { key: self.key.clone(), screen: Box::new(st.screen.clone()) }
            };
            self.broadcast(&msg);
            return Ok(());
        };
        let agent = pane.agent.clone().expect("an agent");
        let width = (*pane.width >= 0.0 && pane.width.is_finite()).then_some(*pane.width as u32);
        let mut screen = parse_screen(lines, width, agent.kind);
        if reads_reply(agent.kind) && self.read_screen_live(lines, width, agent.kind, screen.working) {
            self.live_soon();
        }
        // a menu Inyon is working through itself stays out of the app
        if is_driving(&self.host_id, &self.pane_id) && screen.prompt.is_some() {
            screen.prompt = None;
            screen.input_visible = self.st.borrow().screen.input_visible;
        }
        // the server knows when it's busy; the footer keeps the last word on model, agent and variant
        if agent.kind == AgentKind::Opencode && via_open_code_api(agent.transcript_path.as_deref()) {
            let cwd = Some(pane.cwd.as_str()).filter(|c| !c.is_empty());
            if let Ok(live) = opencode_live(&host, agent.transcript_path.as_deref().unwrap_or(""), cwd).await {
                screen.working = live.working;
                screen.activity = if live.retrying {
                    Some("Retrying".into())
                } else if live.working {
                    Some("Working".into())
                } else {
                    None
                };
                screen.background = Some(live.background);
            }
        }
        {
            let st = self.st.borrow();
            if let Some(q) = st.api_question.as_ref().filter(|q| !q.flow.done() && st.api_permission.is_none()) {
                screen.prompt = Some(q.flow.prompt());
            }
            if let Some(p) = &st.api_permission {
                let mut body = vec![p.title.clone()];
                if let Some(d) = p.detail.as_ref().filter(|d| !d.is_empty()) {
                    body.extend(["╌".to_owned(), d.clone(), "╌".to_owned()]);
                }
                let opt = |n: u64, label: &str, selected: bool| PromptOption { n, label: label.into(), detail: None, selected, checked: None, current: None };
                screen.prompt = Some(ScreenPrompt {
                    body,
                    whole_lines: None,
                    options: vec![opt(1, "Allow once", true), opt(2, "Always allow", false), opt(3, "Reject", false)],
                    tabs: None,
                    footer: None,
                    multi: false,
                    free_text_option: None,
                    navigate: None,
                    input: None,
                    links: None,
                    api: Some(true),
                    confirm: None,
                });
            }
        }
        // a question Codex asked mid-turn sits behind shift+← in its terminal: open it, so it shows
        // here as a card with buttons (not while something is being typed there)
        let opened = self.st.borrow().question_opened_at;
        if agent.kind == AgentKind::Codex
            && screen.questions.is_some_and(|q| q != 0)
            && screen.prompt.is_none()
            && screen.draft.as_deref().is_none_or(str::is_empty)
            && !is_driving(&self.host_id, &self.pane_id)
            && now_ms().saturating_sub(opened) > 8000
        {
            self.st.borrow_mut().question_opened_at = now_ms();
            let (w, host, pane_id) = (self.rc(), host.clone(), self.pane_id.clone());
            spawn(async move {
                if send_keys(&host, &pane_id, &["S-Left"]).await.is_ok() {
                    let _ = w.later(400, |w| w.kick_screen());
                }
            });
        }
        let json = to_json(&screen);
        let changed = {
            let mut st = self.st.borrow_mut();
            if json == st.screen_json {
                false
            } else {
                st.screen_json = json;
                st.screen = screen.clone();
                true
            }
        };
        if changed {
            self.broadcast(&ServerMsg::ChatScreen { key: self.key.clone(), screen: Box::new(screen.clone()) });
            if MIRRORS.contains(&agent.kind) {
                self.sync_draft_from_screen();
            }
            // agy says whether Remote Control is on only on screen; keep the last word after it scrolls away
            let publish = {
                let mut st = self.st.borrow_mut();
                let remote_changed = screen.remote.is_some() && screen.remote.clone().flatten() != st.remote.as_ref().map(|r| r.url.clone());
                if remote_changed {
                    st.remote = screen.remote.clone().flatten().filter(|u| !u.is_empty()).map(|url| RemoteControl { url, ts: date::to_iso(now_ms() as f64).unwrap_or_default() });
                }
                screen.queue.is_some() || !st.info.queue.is_empty() || remote_changed
            };
            if publish {
                self.publish_info();
            }
        }
        Ok(())
    }

    /// Schedules the next screen read at the current pace.
    pub fn next_screen(&self) {
        let stopped = {
            let mut st = self.st.borrow_mut();
            rt::clear(&mut st.screen_timer);
            st.stopped
        };
        if !stopped {
            let t = self.later(if hidden() { QUIET_SCREEN_MS } else { SCREEN_MS }, |w| w.kick_screen());
            self.st.borrow_mut().screen_timer = Some(t);
        }
    }

    pub fn stop(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.stopped = true;
            rt::clear(&mut st.cache_timer);
            rt::clear(&mut st.retry_timer);
        }
        if self.online() {
            self.save();
        }
        if let Some(t) = self.st.borrow_mut().tail.take() {
            t.kill();
        }
        self.stop_live();
        let mut st = self.st.borrow_mut();
        let st = &mut *st;
        for t in [
            &mut st.screen_timer,
            &mut st.flush_timer,
            &mut st.idle_timer,
            &mut st.mirror_timer,
            &mut st.snapshot_timer,
            &mut st.info_timer,
            &mut st.change_timer,
            &mut st.opencode_poll,
        ] {
            rt::clear(t);
        }
    }
}

/// Waits for a stream to end, or kills it after `ms`: its exit code, or `timeout` then.
async fn wait_stream(t: &StreamHandle, ms: u64, timeout: i32) -> i32 {
    let done = Deferred::<i32>::new();
    let watchdog = {
        let (t, d) = (t.clone(), done.clone());
        set_timeout(ms, move || {
            t.kill();
            d.resolve(timeout);
        })
    };
    {
        let (exited, d) = (t.exited.clone(), done.clone());
        spawn(async move { d.resolve(exited.wait().await) });
    }
    let code = done.wait().await;
    watchdog.clear();
    code
}

fn empty_info(host_id: &str, pane_id: &str) -> ChatInfo {
    serde_json::from_value(serde_json::json!({
        "hostId": host_id, "paneId": pane_id, "running": false, "queue": [], "tasks": [], "subagents": [], "artifacts": [],
        "stats": { "turns": 0, "compactions": 0 },
    }))
    .expect("an empty chat info")
}

fn reset_msg(key: &str, st: &State) -> ServerMsg {
    let start = st.items.len().saturating_sub(PAGE);
    ServerMsg::ChatReset { key: key.to_owned(), info: Box::new(st.info.clone()), items: st.items[start..].to_vec(), has_more: start > 0 || older_in_file(st) }
}

fn older_in_file(st: &State) -> bool {
    if st.reader.is_none() || st.transcript.is_none() {
        return false;
    }
    if let Some(o) = st.older_from {
        return o > 0 || st.held_history.as_ref().is_some_and(|h| !h.is_empty());
    }
    let first = st.items.first().map_or(0, ChatItem::seq);
    first > 0 && first < TAIL_SEQ
}

/// Antigravity's queue is only on screen.
fn current_queue(st: &State) -> Vec<String> {
    if !st.meta.queue.is_empty() {
        st.meta.queue.clone()
    } else {
        st.screen.queue.clone().unwrap_or_default()
    }
}

fn last40<'a, T: Clone + 'a>(it: impl Iterator<Item = &'a T>) -> Vec<T> {
    let all: Vec<T> = it.cloned().collect();
    all[all.len().saturating_sub(40)..].to_vec()
}

/// The ids a conversation item is known by, for matching it to the live copy written before it.
/// What's being written now: Codex's live items, then the reply read off the screen.
fn live_items(st: &State) -> Vec<LiveItem> {
    let mut items = st.live.list();
    items.extend(st.screen_live.clone());
    items
}

/// The texts of the latest replies on record, which live text read off the screen is checked against.
fn recent_replies(items: &[ChatItem]) -> Vec<&str> {
    use inyon_protocol::AssistantBlock;
    items
        .iter()
        .rev()
        .take(60)
        .filter_map(|it| match it {
            ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn record_ids(it: &ChatItem) -> Vec<Option<String>> {
    use inyon_protocol::AssistantBlock;
    let msg_id = match it {
        ChatItem::Assistant { msg_id, .. } => Some(msg_id.clone()),
        _ => None,
    };
    let tool = match it {
        ChatItem::Assistant { block: AssistantBlock::ToolUse { id, .. }, .. } => Some(id.clone()),
        _ => None,
    };
    let result = match it {
        ChatItem::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
        _ => None,
    };
    vec![Some(it.uuid().to_owned()), msg_id, tool, result]
}

/// The record's sub-agents, plus any the live events know about that the record hasn't written yet.
fn merged_agents(m: &TranscriptMeta, live: &CodexLiveState) -> Vec<SubagentInfo> {
    let mut out = m.subagents.clone();
    for a in live.agents.values() {
        if !out.contains_key(&a.thread_id) {
            out.insert(
                a.thread_id.clone(),
                serde_json::from_value(
                    serde_json::json!({ "toolUseId": a.thread_id, "type": "codex", "description": a.description, "status": a.status, "startedAt": a.started_at }),
                )
                .expect("a subagent"),
            );
        }
    }
    out.into_values().collect()
}

/// Every open chat, by its key, and the state of chats closed a moment ago.
#[derive(Default)]
pub struct Watchers {
    map: RefCell<IndexMap<String, Rc<ChatWatcher>>>,
    parked: RefCell<IndexMap<String, Parked>>,
}

/// This thread's watchers.
#[track_caller]
pub fn watchers() -> WatchersRef {
    crate::platform::rt::assert_core_thread("the chat watchers");
    WatchersRef(std::marker::PhantomData)
}

/// A handle to this thread's [`Watchers`]. It can't be sent to another thread, where it would
/// reach that thread's (empty) watchers.
#[derive(Clone, Copy)]
pub struct WatchersRef(std::marker::PhantomData<Rc<()>>);

impl WatchersRef {
    fn with<R>(&self, f: impl FnOnce(&Watchers) -> R) -> R {
        WATCHERS.with(f)
    }

    pub fn get(&self, host_id: &str, pane_id: &str) -> Rc<ChatWatcher> {
        let key = chat_key(host_id, pane_id);
        if let Some(w) = self.with(|w| w.map.borrow().get(&key).cloned()) {
            return w;
        }
        let p = self.with(|w| w.parked.borrow_mut().shift_remove(&key)).filter(|p| now_ms().saturating_sub(p.at) < PARK_MS);
        let w = ChatWatcher::new(host_id, pane_id, p);
        self.with(|ws| ws.map.borrow_mut().insert(key, w.clone()));
        w
    }

    /// Reads open chats' screens rarely while the app is in the background, and at once on return.
    pub fn quiet(&self, on: bool) {
        if hidden() == on {
            return;
        }
        HIDDEN.with(|h| h.set(on));
        for w in self.all() {
            if on {
                w.next_screen();
            } else {
                w.kick_screen();
            }
        }
    }

    fn all(&self) -> Vec<Rc<ChatWatcher>> {
        self.with(|w| w.map.borrow().values().cloned().collect())
    }

    /// Saves every open chat now (the app is going to the background and may not come back).
    pub fn save_all(&self) {
        for w in self.all() {
            w.save();
        }
    }

    /// Forgets the chats closed a moment ago, as a restart would (tests).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn clear_parked(&self) {
        self.with(|w| w.parked.borrow_mut().clear());
    }

    pub fn peek(&self, host_id: &str, pane_id: &str) -> Option<Rc<ChatWatcher>> {
        self.with(|w| w.map.borrow().get(&chat_key(host_id, pane_id)).cloned())
    }

    pub fn stop(&self, key: &str) {
        let Some(w) = self.with(|ws| ws.map.borrow().get(key).cloned()) else { return };
        let p = w.park();
        w.stop();
        self.with(|ws| ws.map.borrow_mut().shift_remove(key));
        let Some(p) = p else { return };
        self.with(|ws| {
            let mut parked = ws.parked.borrow_mut();
            parked.insert(key.to_owned(), p);
            while parked.len() > PARKED_MAX {
                parked.shift_remove_index(0);
            }
        });
    }
}

/// How long to wait before following a transcript again after its tail ended `retries` times in a
/// row: a second, doubling, never more than 30 s (`Math.min(30_000, 1000 * 2 ** retries)`).
fn retry_ms(retries: u32) -> u64 {
    30_000u64.min(1000 * 2u64.pow(retries.min(5)))
}

/// The registry's events, as the TS Watchers constructor subscribes to them.
pub fn on_registry_event(e: &RegistryEvent) {
    match e {
        RegistryEvent::Overview => {
            for w in watchers().all() {
                w.sync();
            }
        }
        RegistryEvent::Hook { host, pane, .. } => {
            if let Some(w) = watchers().peek(host, pane) {
                w.kick_screen();
            }
        }
        RegistryEvent::PaneOutput { host, pane } => {
            if let Some(w) = watchers().peek(host, pane) {
                w.screen_changed();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
