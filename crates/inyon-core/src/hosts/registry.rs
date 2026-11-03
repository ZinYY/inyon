//! Port of hosts/registry.ts: every machine, polled for its tmux sessions and the agents in them,
//! with live updates from tmux control mode and the hook log, notifications when an agent waits or
//! finishes, and the last state seen kept for machines that are away.
//!
//! One registry per thread, as the TS has one per process; get it with [`registry`]. Its state
//! lives in `RefCell`s that are never borrowed across an await or while a listener runs.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use futures_util::future::join_all;
use indexmap::{IndexMap, IndexSet};
use inyon_protocol::{
    agents, AccountLimits, AgentKind, AgentPaneInfo, AgentState, AgentStatus, HostConfig, HostKind, HostState, HostStatus, IntegrationState, MappedBy, PaneInfo, QuickOption,
    RateWindow, ScreenPrompt, ScreenState, SessionInfo, TmuxState, WindowInfo,
};
use serde_json::Value;

use super::coalesce::coalesce;
use super::failure::{failure_text, turn_failure};
use super::host::{shq, Host, StreamHandle};
use super::http_error::{HubError, HubResult};
use super::names::{bare_title, name_taken, owns_title, pending_name, session_name};
use super::node_link;
use super::node_update::node_updates;
use super::turns::{Shown, Turns};
use crate::agents::antigravity::{agy_status, agy_transcript};
use crate::agents::claude_usage::{fetch_claude_usage, AccountUsage};
use crate::agents::codex::{codex_title, find_codex_rollout, read_codex_status, CodexProc};
use crate::agents::codex_limits;
use crate::agents::copilot::{find_copilot_session, CopilotProc};
use crate::agents::gemini::{find_gemini_session, GeminiProc};
use crate::agents::opencode::{opencode_endpoint, via_open_code_api};
use crate::agents::opencode_api::opencode_catalog_cached;
use crate::agents::opencode_controls::opencode_live;
use crate::agents::opencode_db::{find_open_code_session, ocdb_session, OpenCodeProc, OCDB};
use crate::agents::opencode_models::{model_from_footer, opencode_cli_models, peek_open_code_models, without_provider};
use crate::agents::status::claude_status;
use crate::claude::agent_hooks::{install_agent_hooks, HOOKED_AGENTS};
use crate::claude::chat_cache::prune_chats;
use crate::claude::driving::is_driving;
use crate::claude::screen::{parse_screen, prompt_summary, prompt_whole};
use crate::claude::transcript_tail::until_stdin_closes;
use crate::clock::now_ms;
use crate::config::{load_hosts, load_settings, save_hosts};
use crate::js::json::{self, Js};
use crate::js::{date, JsStr};
use crate::platform::console;
use crate::platform::rt::{self, set_timeout, sleep, spawn, Timer};
use crate::platform::store::{key_part, kv};
use crate::re;
use crate::tmux::grid;
use crate::tmux::tmux::{capture_panes, find_transcript, probe, AgentProc, HookPaneState, LiveTmux, ProbeResult, TmuxProbe, TranscriptProc};

/// Puts the agent windows nobody's tmux client shows on the wide grid (see tmux::grid). psmux
/// sizes its windows its own way, so Windows is left alone.
fn rest_windows(rt: &Rt, host: &Host, p: &ProbeResult) {
    if p.os == "Windows" {
        return;
    }
    let agent_windows: HashSet<String> =
        p.sessions.iter().flat_map(|s| &s.windows).filter(|w| w.panes.iter().any(|x| p.agent_procs.contains_key(&x.id))).map(|w| w.id.clone()).collect();
    let windows = grid::to_rest(&p.unwatched, &agent_windows, &mut rt.borrow_mut().rested);
    if !windows.is_empty() {
        let (rt, host) = (rt.clone(), host.clone());
        spawn(async move {
            // a call that didn't reach tmux is tried again on the next poll
            if !host.run(&grid::rest_command(&windows)).await.is_ok_and(|r| r.code == 0) {
                let rested = &mut rt.borrow_mut().rested;
                for w in &windows {
                    rested.remove(w);
                }
            }
        });
    }
}

/// Fallback poll; tmux control mode delivers changes as they happen.
const POLL_MS: u64 = 4000;
/// while nobody is looking (the phone app in the background), poll much less: it costs battery and data
const QUIET_POLL_MS: u64 = 20000;
const MAX_BACKOFF_MS: u64 = 20000;
/// Someone who pressed a key in a tmux client on the pane this recently is watching it: its
/// finish isn't announced.
const TYPING_S: f64 = 60.0;

thread_local! {
    static POLL: Cell<u64> = const { Cell::new(POLL_MS) };
    /// `navigator.onLine`: the phone app says when it has no network; the hub always has one
    static ONLINE: Cell<bool> = const { Cell::new(true) };
}

fn poll_ms() -> u64 {
    POLL.with(Cell::get)
}

/// The phone has a network (true) or not (false); no "offline" alerts without one.
pub fn set_online(on: bool) {
    ONLINE.with(|o| o.set(on));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    Waiting,
    Done,
    Offline,
    Online,
}

impl NotifyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyKind::Waiting => "waiting",
            NotifyKind::Done => "done",
            NotifyKind::Offline => "offline",
            NotifyKind::Online => "online",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegistryEvent {
    Overview,
    Notify {
        host: String,
        pane: String,
        kind: NotifyKind,
        title: String,
        body: String,
        agent: Option<AgentKind>,
    },
    Hook {
        host: String,
        pane: String,
        event: Value,
    },
    PaneOutput {
        host: String,
        pane: String,
    },
    /// a rename waits for this agent, which is idle now
    RenameDue {
        host: String,
        pane: String,
    },
    /// the companion agent finished a turn (in place of its Done notice); `failure` says why it stopped
    CompanionDone {
        host: String,
        pane: String,
        failure: Option<String>,
    },
}

/// agent pid → transcript found without hooks (by folder, resume id or Codex rollout)
#[derive(Debug, Clone)]
struct Mapped {
    path: Option<String>,
    at: u64,
    title: String,
}

/// agent pid → status read from its transcript (Codex)
#[derive(Debug, Clone)]
struct CachedStatus {
    status: Option<AgentStatus>,
    permission: Option<String>,
    at: u64,
}

/// Plan limits for the signed-in Claude account, read the way /usage does. `at` is the last
/// attempt, `ok_at` the last success, whose numbers are kept through errors.
#[derive(Debug, Clone, Default)]
struct Usage {
    u: AccountUsage,
    account: Option<String>,
    ok_at: Option<u64>,
}

/// Codex's plan limits, asked of its app-server. `at` is the last attempt; the last numbers it
/// gave are kept through errors.
#[derive(Debug, Clone, Default)]
struct CodexLimits {
    limits: Option<AccountLimits>,
    at: u64,
}

/// What the registry keeps per machine.
pub struct HostRuntime {
    /// each agent's state when the machine went offline, kept to compare with once it's back
    before_outage: Option<HashMap<String, (AgentKind, AgentState)>>,
    /// the "offline" notification went out for the current outage
    offline_notified: bool,
    /// last snapshot written to disk (JSON of the sessions) and when
    saved_snapshot: Option<String>,
    saved_at: Option<u64>,
    pub host: Host,
    pub state: HostState,
    /// the last probe, as annotated
    pub probe: Option<Rc<RefCell<ProbeResult>>>,
    timer: Option<Timer>,
    failures: u32,
    polling: bool,
    poll_again: bool,
    events: Option<StreamHandle>,
    cwd_map: HashMap<i64, Mapped>,
    status_cache: HashMap<i64, CachedStatus>,
    stopped: bool,
    /// last time the agy status line was put back
    agy_healed_at: Option<u64>,
    /// when an agent's missing hook was last added (see heal_integration)
    hooks_healed_at: Option<u64>,
    live: Option<Rc<LiveTmux>>,
    /// panes with fresh output, re-read on a short debounce
    dirty: IndexSet<String>,
    dirty_timer: Option<Timer>,
    /// last screen per agent pane, for deciding whether a restart is safe
    pub screens: HashMap<String, ScreenState>,
    /// when the integration was last put back automatically
    healed_at: Option<u64>,
    /// sessions already resumed automatically (once each, so a failure can't loop)
    resumed: HashSet<String>,
    /// sessions already reconnected through their project settings, and when
    reconnected: IndexMap<String, u64>,
    usage: Option<Usage>,
    usage_loading: bool,
    codex: Option<CodexLimits>,
    codex_loading: bool,
    /// windows asked to rest on the wide grid, at the size each had then (see tmux::grid)
    rested: HashMap<String, (u32, u32)>,
}

pub type Rt = Rc<RefCell<HostRuntime>>;

impl HostRuntime {
    /// A test's machine that no poll reads again (the TS tests stub `registry.refresh`).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn hold_polls(&mut self) {
        self.stopped = true;
    }

    pub fn new(host: Host, state: HostState) -> Self {
        HostRuntime {
            before_outage: None,
            offline_notified: false,
            saved_snapshot: None,
            saved_at: None,
            host,
            state,
            probe: None,
            timer: None,
            failures: 0,
            polling: false,
            poll_again: false,
            events: None,
            cwd_map: HashMap::new(),
            status_cache: HashMap::new(),
            stopped: false,
            agy_healed_at: None,
            hooks_healed_at: None,
            live: None,
            dirty: IndexSet::new(),
            dirty_timer: None,
            screens: HashMap::new(),
            healed_at: None,
            resumed: HashSet::new(),
            rested: HashMap::new(),
            reconnected: IndexMap::new(),
            usage: None,
            usage_loading: false,
            codex: None,
            codex_loading: false,
        }
    }
}

/// a transcript this far ahead of the status line means the session stopped reporting
const STALL_S: f64 = 90.0;
/// account limits: refresh every three minutes, retry a failure after five, ignore after twenty
// the usage endpoint rate-limits; Claude's own /usage is only asked when you open it
const USAGE_EVERY_MS: u64 = 3 * 60_000;
const USAGE_RETRY_MS: u64 = 5 * 60_000;
const USAGE_STALE_MS: u64 = 20 * 60_000;
/// Codex's limits: asked every five minutes while someone's looking (each ask starts its app-server)
const CODEX_EVERY_MS: u64 = 5 * 60_000;

/// The plan limits a machine shows: Claude's account (as /usage reads it) and Codex's, each while
/// it's fresh and has a window to show.
fn limits_list(claude: Option<&Usage>, account: Option<&str>, codex: Option<&CodexLimits>, now: u64) -> Option<Vec<AccountLimits>> {
    let fresh = |at: u64| now.saturating_sub(at) <= USAGE_STALE_MS;
    let mut out = vec![];
    if let Some(u) = claude.filter(|u| u.account.as_deref() == account) {
        if let Some(at) = u.ok_at.filter(|at| *at != 0 && fresh(*at)) {
            out.push(AccountLimits {
                kind: AgentKind::Claude,
                five_hour: u.u.five_hour.clone(),
                seven_day: u.u.seven_day.clone(),
                plan: u.u.plan.clone(),
                credits: None,
                resets: None,
                updated_at: at,
            });
        }
    }
    if let Some(l) = codex.and_then(|c| c.limits.clone()).filter(|l| fresh(l.updated_at)) {
        out.push(l);
    }
    out.retain(|l| l.five_hour.is_some() || l.seven_day.is_some());
    (!out.is_empty()).then_some(out)
}
/// don't fight a tool that keeps rewriting settings.json
const HEAL_EVERY_MS: u64 = 10 * 60_000;

/// a machine that stops answering for this long gets an "offline" notification
const OFFLINE_NOTIFY_MS: u64 = 60_000;

/// What `offline_alert_due` looks at.
#[derive(Debug, Clone, Copy)]
pub struct OfflineAlert {
    pub since: Option<u64>,
    pub notified: bool,
    pub now: u64,
    pub held: bool,
    pub held_until: u64,
    pub online: bool,
}

/// Whether to say a machine is offline. On the phone, the app's own connection drops whenever it's in
/// the background or the phone has no signal, which says nothing about the machine: no alerts then,
/// and the minute starts over when the app is back.
pub fn offline_alert_due(o: OfflineAlert) -> bool {
    let Some(since) = o.since.filter(|s| *s != 0) else { return false };
    if o.notified || o.held || !o.online {
        return false;
    }
    o.now as f64 - since.max(o.held_until) as f64 >= OFFLINE_NOTIFY_MS as f64
}

/// `JSON.stringify` of a value as the wire carries it.
fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).map(|v| crate::js::stringify(&v)).unwrap_or_default()
}

/// Whether a poll changed anything the apps show, so they're sent a new overview. Every poll moves
/// the clock (`updatedAt`, `lastSeen`) and the response time; sent for that alone, each app got a
/// full overview every 4 s. `lastSeen` only shows while a machine is offline, and the overview
/// sent when it goes offline carries the latest one.
pub fn overview_changed(prev: &HostState, next: &HostState) -> bool {
    let still = |s: &HostState| {
        let mut s = s.clone();
        s.updated_at = 0;
        s.latency_ms = Some(0);
        s.last_seen = Some(if s.last_seen.is_some_and(|l| l != 0) { 1 } else { 0 });
        to_json(&s)
    };
    still(prev) != still(next)
}

fn snapshot_key(id: &str) -> String {
    format!("snapshots/{}", key_part(id))
}

/// The last state seen from a machine, so it still shows its sessions while it's unreachable.
fn load_snapshot(id: &str) -> Option<HostState> {
    kv::get(&snapshot_key(id))
}

fn save_snapshot(state: &HostState) {
    let mut s = state.clone();
    s.config = public_config(&s.config);
    if let Err(e) = kv::set(&snapshot_key(&state.config.id), &s) {
        console::error(&format!("snapshot: {e}"));
    }
}

/// Sessions as last seen: nothing is known to be running or waiting any more.
fn offline_sessions(sessions: &[SessionInfo]) -> Vec<SessionInfo> {
    let mut out = sessions.to_vec();
    for p in out.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.iter_mut()) {
        if let Some(a) = p.agent.as_mut() {
            a.state = AgentState::Unknown;
            a.waiting_for = None;
        }
    }
    out
}

/// A pane's title without the spinner or icon an agent puts in front.
pub fn pane_title(p: &PaneInfo) -> String {
    re!(r"^[^\w\s]\s+", "u").replacen(&p.title, 1, "").jtrim().to_owned()
}

/// What a machine reached through inyon is waiting on.
fn waiting_for(c: &HostConfig) -> String {
    node_link::why(&c.id).unwrap_or_else(|| {
        if c.kind == HostKind::Link {
            format!("Connecting to {}…", c.name)
        } else {
            "Waiting for the machine to connect (is inyon running?)".into()
        }
    })
}

fn reached_through_inyon(c: &HostConfig) -> bool {
    matches!(c.kind, HostKind::Node | HostKind::Link)
}

/// A pane found by [`Registry::find_pane`]: copies of it and of what holds it.
#[derive(Debug, Clone)]
pub struct FoundPane {
    pub session: SessionInfo,
    pub window: WindowInfo,
    pub pane: PaneInfo,
    pub home: Option<String>,
}

type Listener = Rc<dyn Fn(&RegistryEvent)>;

/// Every pane of a machine's sessions.
fn panes_mut(sessions: &mut [SessionInfo]) -> impl Iterator<Item = &mut PaneInfo> {
    sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.iter_mut())
}

fn panes(sessions: &[SessionInfo]) -> impl Iterator<Item = &PaneInfo> {
    sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.iter())
}

pub struct Registry {
    hosts: RefCell<IndexMap<String, Rt>>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_listener: Cell<u64>,
    /// offline alerts are held while the phone app is hidden; this is when they were last released
    alerts_held: Cell<bool>,
    alerts_from: Cell<u64>,
    /// several machines and panes change within the same moment; the apps hear one overview for it
    overview_soon: Rc<dyn Fn()>,
    /// How long a pane stays idle with nothing left in the background before a turn that ended
    /// with background work counts as finished: when a subagent reports back, the main turn often
    /// goes on a moment later, and that isn't two finishes.
    pub done_grace: Cell<u64>,
    done_timers: RefCell<HashMap<String, Timer>>,
}

thread_local! {
    static REGISTRY: Rc<Registry> = Rc::new(Registry::new());
    /// Turns that go on while the screen reads idle (see turns.rs).
    static TURNS: RefCell<Turns> = RefCell::new(Turns::new());
}

/// This thread's registry.
#[track_caller]
pub fn registry() -> Rc<Registry> {
    crate::platform::rt::assert_core_thread("the registry");
    REGISTRY.with(Rc::clone)
}

fn turn_key(host: &str, pane: &str) -> String {
    crate::js::stringify(&serde_json::json!([host, pane]))
}

/// Whether a person typed in a tmux client showing the pane within the last minute, by the
/// machine's own clock.
fn typed_lately(rt: &Rt, pane: &str) -> bool {
    let r = rt.borrow();
    r.probe.as_ref().is_some_and(|p| {
        let p = p.borrow();
        p.keys_at.get(pane).is_some_and(|at| p.now - at <= TYPING_S)
    })
}

impl Registry {
    fn new() -> Self {
        Registry {
            hosts: RefCell::new(IndexMap::new()),
            listeners: RefCell::new(vec![]),
            next_listener: Cell::new(0),
            alerts_held: Cell::new(false),
            alerts_from: Cell::new(0),
            overview_soon: coalesce(|| registry().dispatch(&RegistryEvent::Overview), 100),
            done_grace: Cell::new(3000),
            done_timers: RefCell::new(HashMap::new()),
        }
    }

    pub fn start(&self) {
        for mut c in load_hosts() {
            // tokens made before they expired: their day starts now
            if c.kind == HostKind::Node && c.paired_at.is_none() && c.token_hash.is_some() && c.token_issued_at.is_none() {
                c.token_issued_at = Some(now_ms());
            }
            // nothing is saved here, so nothing can fail
            let _ = self.add(c, false);
        }
        // a machine running inyon (re)connected or dropped: re-probe it right away
        node_link::on_change(|id, up| {
            let me = registry();
            let Some(rt) = me.get(id) else { return };
            if !up {
                let live = {
                    let mut r = rt.borrow_mut();
                    r.events = None;
                    r.live.clone()
                };
                if let Some(l) = live {
                    l.stop();
                }
                // a blip (Wi-Fi, a VPN finding a new path) is over in seconds: its sessions stay as
                // they are meanwhile, and the machine is down only if it isn't back by then
                if node_link::reconnecting(id) {
                    me.refresh(id, node_link::RECONNECT_GRACE_MS + 100);
                    return;
                }
                let w = waiting_for(&rt.borrow().host.config());
                me.mark_down(&rt, HostStatus::Connecting, &w);
                me.emit(RegistryEvent::Overview);
            }
            me.refresh(id, if up { 100 } else { 1000 });
        });
        // an update of a machine's inyon started, progressed or finished
        let _ = node_updates().on_change(|| registry().emit(RegistryEvent::Overview));
    }

    /// Hears every event; call the returned function to stop.
    pub fn on(&self, f: impl Fn(&RegistryEvent) + 'static) -> impl Fn() {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(f)));
        move || registry().listeners.borrow_mut().retain(|(i, _)| *i != id)
    }

    pub(crate) fn emit(&self, e: RegistryEvent) {
        if e == RegistryEvent::Overview {
            return (self.overview_soon)();
        }
        self.dispatch(&e);
    }

    /// To the chat watchers and the actions first, as the TS modules registered first, then to
    /// everyone listening. A listener removed by an earlier one isn't called.
    fn dispatch(&self, e: &RegistryEvent) {
        crate::claude::watcher::on_registry_event(e);
        crate::claude::actions::on_registry_event(e);
        let all: Vec<(u64, Listener)> = self.listeners.borrow().clone();
        for (id, f) in all {
            if self.listeners.borrow().iter().any(|(i, _)| *i == id) {
                f(e);
            }
        }
    }

    pub fn configs(&self) -> Vec<HostConfig> {
        self.hosts.borrow().values().map(|r| r.borrow().host.config()).collect()
    }

    pub fn overview(&self) -> Vec<HostState> {
        let all: Vec<Rt> = self.hosts.borrow().values().cloned().collect();
        all.iter()
            .map(|r| {
                let r = r.borrow();
                let mut s = r.state.clone();
                s.config = public_config(&r.state.config);
                s.node_update = node_updates().view(&r.state);
                s
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Rt> {
        self.hosts.borrow().get(id).cloned()
    }

    pub fn host(&self, id: &str) -> HubResult<Host> {
        self.get(id).map(|r| r.borrow().host.clone()).ok_or_else(|| HubError::http(404, "Unknown machine"))
    }

    pub fn find_pane(&self, host_id: &str, pane_id: &str) -> Option<FoundPane> {
        let rt = self.get(host_id)?;
        let r = rt.borrow();
        let home = r.probe.as_ref().map(|p| p.borrow().home.clone());
        for s in &r.state.sessions {
            for w in &s.windows {
                if let Some(p) = w.panes.iter().find(|p| p.id == pane_id) {
                    return Some(FoundPane { session: s.clone(), window: w.clone(), pane: p.clone(), home });
                }
            }
        }
        None
    }

    /// The agent the last look at a pane found there.
    pub fn pane_agent(&self, host_id: &str, pane_id: &str) -> Option<AgentPaneInfo> {
        let rt = self.get(host_id)?;
        let r = rt.borrow();
        let found = panes(&r.state.sessions).find(|p| p.id == pane_id).and_then(|p| p.agent.clone());
        found
    }

    /// Changes a pane of the machine's current state in place (the TS mutates what `findPane`
    /// returned). False when there's no such pane.
    pub fn update_pane(&self, host_id: &str, pane_id: &str, f: impl FnOnce(&mut PaneInfo)) -> bool {
        let Some(rt) = self.get(host_id) else { return false };
        let mut r = rt.borrow_mut();
        let found = match panes_mut(&mut r.state.sessions).find(|p| p.id == pane_id) {
            Some(p) => {
                f(p);
                true
            }
            None => false,
        };
        found
    }

    /// Fails, with the machine in the list but not polled, when the list can't be saved (as the TS
    /// throws from `saveHosts`).
    pub fn add(&self, config: HostConfig, persist: bool) -> HubResult<()> {
        // start from the last state seen, so a machine that's unreachable right now still shows
        // its sessions (marked offline) until it answers again
        let state = match load_snapshot(&config.id) {
            Some(snap) => HostState {
                config: config.clone(),
                status: HostStatus::Connecting,
                error: None,
                latency_ms: None,
                sessions: offline_sessions(&snap.sessions),
                last_seen: snap.last_seen.or(Some(snap.updated_at)),
                offline_since: snap.offline_since,
                ..snap
            },
            None => HostState {
                config: config.clone(),
                status: HostStatus::Connecting,
                error: None,
                hostname: None,
                integration: IntegrationState::Unknown,
                agy_integration: None,
                tmux: TmuxState::Unknown,
                agents: vec![],
                signed_in: None,
                os: None,
                machine_kind: None,
                node_version: None,
                node_warning: None,
                node_update: None,
                latency_ms: None,
                sessions: vec![],
                updated_at: 0,
                last_seen: None,
                offline_since: None,
                limits: None,
            },
        };
        let id = config.id.clone();
        let host = Host::new(config);
        let rt: Rt = Rc::new(RefCell::new(HostRuntime::new(host.clone(), state)));
        let (a, b) = (Rc::downgrade(&rt), id.clone());
        let live = LiveTmux::new(
            host,
            move |pane| {
                if let Some(rt) = a.upgrade() {
                    registry().on_pane_output(&rt, pane);
                }
            },
            move || registry().refresh(&b, 150),
        );
        rt.borrow_mut().live = Some(Rc::new(live));
        self.hosts.borrow_mut().insert(id, rt.clone());
        if persist {
            self.persist()?;
        }
        let r = rt.clone();
        spawn(async move { registry().poll(r).await });
        self.emit(RegistryEvent::Overview);
        Ok(())
    }

    pub fn update(&self, id: &str, patch: impl FnOnce(&mut HostConfig)) -> HubResult<()> {
        let rt = self.get(id).ok_or_else(|| HubError::http(404, "Unknown machine"))?;
        let mut config = rt.borrow().host.config();
        patch(&mut config);
        config.id = id.to_owned();
        self.remove(id, false)?;
        self.add(config, true)
    }

    /// Changes a machine's settings in place, its state's copy too.
    fn edit_config(rt: &Rt, f: impl FnOnce(&mut HostConfig)) {
        let r = rt.borrow_mut();
        r.host.update_config(f);
        let c = r.host.config();
        drop(r);
        rt.borrow_mut().state.config = c;
    }

    /// Records something about a machine that doesn't change how it's reached right now.
    pub fn note(&self, id: &str, patch: impl FnOnce(&mut HostConfig)) -> HubResult<()> {
        let Some(rt) = self.get(id) else { return Ok(()) };
        Self::edit_config(&rt, patch);
        self.persist()
    }

    /// A fresh pairing token for a node: the old one stops working and it waits to be paired again.
    pub fn set_node_token(&self, id: &str, token_hash: &str) -> HubResult<()> {
        let rt = self.get(id).filter(|r| r.borrow().host.config().kind == HostKind::Node).ok_or_else(|| HubError::http(404, "Unknown machine"))?;
        Self::edit_config(&rt, |c| {
            c.token_hash = Some(token_hash.to_owned());
            c.token_issued_at = Some(now_ms());
            c.paired_at = None;
        });
        self.persist()?;
        self.emit(RegistryEvent::Overview);
        Ok(())
    }

    /// A node connected with its token for the first time: it is no longer waiting to be paired.
    pub fn mark_paired(&self, id: &str) -> HubResult<()> {
        let Some(rt) = self.get(id) else { return Ok(()) };
        if rt.borrow().host.config().paired_at.is_some_and(|p| p != 0) {
            return Ok(());
        }
        Self::edit_config(&rt, |c| c.paired_at = Some(now_ms()));
        self.persist()?;
        self.emit(RegistryEvent::Overview);
        Ok(())
    }

    pub fn remove(&self, id: &str, persist: bool) -> HubResult<()> {
        let Some(rt) = self.get(id) else { return Ok(()) };
        let (events, live) = {
            let mut r = rt.borrow_mut();
            r.stopped = true;
            rt::clear(&mut r.timer);
            rt::clear(&mut r.dirty_timer);
            (r.events.clone(), r.live.clone())
        };
        if let Some(e) = events {
            e.kill();
        }
        if let Some(l) = live {
            l.stop();
        }
        self.hosts.borrow_mut().shift_remove(id);
        if persist {
            kv::del(&snapshot_key(id));
            prune_chats(Some(id));
            self.persist()?;
            self.emit(RegistryEvent::Overview);
        }
        Ok(())
    }

    /// Saves the machine list; a failure goes to whoever changed it (an API call answers 500 with it).
    fn persist(&self) -> HubResult<()> {
        save_hosts(&self.configs()).map_err(HubError::plain)
    }

    /// Remember whether to put the Claude integration back if it disappears.
    pub fn set_keep_integration(&self, id: &str, keep: bool) -> HubResult<()> {
        let Some(rt) = self.get(id) else { return Ok(()) };
        if rt.borrow().host.config().keep_integration == Some(keep) {
            return Ok(());
        }
        Self::edit_config(&rt, |c| c.keep_integration = Some(keep));
        self.persist()
    }

    /// A prompt in this Claude pane was answered from Inyon: show it working until the turn ends.
    pub fn turn_goes_on(&self, host_id: &str, pane_id: &str) {
        TURNS.with(|t| t.borrow_mut().start(&turn_key(host_id, pane_id), now_ms()));
    }

    /// The turn was cancelled from Inyon (Esc): believe the screen again.
    pub fn turn_ended(&self, host_id: &str, pane_id: &str) {
        TURNS.with(|t| t.borrow_mut().end(&turn_key(host_id, pane_id), now_ms()));
    }

    /// The agent process found in a pane by the last probe.
    pub fn agent_proc(&self, host_id: &str, pane_id: &str) -> Option<AgentProc> {
        let rt = self.get(host_id)?;
        let r = rt.borrow();
        let p = r.probe.as_ref()?.borrow();
        p.agent_procs.get(pane_id).cloned()
    }

    /// Re-read cached agent numbers and settings on the next poll (after changing them).
    pub fn forget_status(&self, id: &str) {
        if let Some(rt) = self.get(id) {
            rt.borrow_mut().status_cache.clear();
        }
    }

    /// Polls every 20 s instead of 4 s while no one is looking; the next poll picks it up.
    pub fn quiet(&self, on: bool) {
        POLL.with(|p| p.set(if on { QUIET_POLL_MS } else { POLL_MS }));
    }

    /// Whether machines are polled every 20 s (no one looking) rather than every 4.
    pub fn is_quiet(&self) -> bool {
        POLL.with(Cell::get) == QUIET_POLL_MS
    }

    /// The phone app went to the background (true) or came back with a network (false).
    pub fn hold_alerts(&self, on: bool) {
        self.alerts_held.set(on);
        if !on {
            self.alerts_from.set(now_ms());
        }
    }

    /// Re-poll a machine soon (after an action or a hook event).
    pub fn refresh(&self, id: &str, delay: u64) {
        let Some(rt) = self.get(id) else { return };
        {
            let mut r = rt.borrow_mut();
            if r.stopped {
                return;
            }
            if r.polling {
                r.poll_again = true;
                return;
            }
            rt::clear(&mut r.timer);
        }
        // not borrowed while the timer is set: a runtime may run a zero delay at once
        let timer = Self::poll_later(&rt, delay);
        rt.borrow_mut().timer = Some(timer);
    }

    fn poll_later(rt: &Rt, ms: u64) -> Timer {
        let rt = rt.clone();
        set_timeout(ms, move || spawn(async move { registry().poll(rt).await }))
    }

    /// Reads every machine again now (pull to refresh); settles once they've answered, or after `ms`.
    pub async fn refresh_all(&self, ms: u64) {
        let ids: Vec<String> = self.hosts.borrow().keys().cloned().collect();
        for id in ids {
            self.refresh(&id, 0);
        }
        let until = now_ms() + ms;
        sleep(50).await;
        while now_ms() < until
            && self.hosts.borrow().values().any(|rt| {
                let r = rt.borrow();
                !r.stopped && (r.polling || r.poll_again)
            })
        {
            sleep(100).await;
        }
    }

    async fn poll(&self, rt: Rt) {
        let (stopped, host) = {
            let r = rt.borrow();
            (r.stopped, r.host.clone())
        };
        if stopped {
            return;
        }
        let config = host.config();
        if reached_through_inyon(&config) && !node_link::is_up(&config.id) {
            if node_link::reconnecting(&config.id) {
                let t = Self::poll_later(&rt, 1000);
                rt.borrow_mut().timer = Some(t);
                return;
            }
            self.mark_down(&rt, HostStatus::Connecting, &waiting_for(&config));
            let t = Self::poll_later(&rt, 5000);
            rt.borrow_mut().timer = Some(t);
            return;
        }
        rt.borrow_mut().polling = true;
        let mut next = poll_ms();
        match self.poll_once(&rt, &host).await {
            Ok(()) => {}
            // the read in flight when the connection dropped: not a fault of the machine's
            Err(_) if reached_through_inyon(&config) && node_link::reconnecting(&config.id) => next = 1000,
            Err(error) => {
                let failures = {
                    let mut r = rt.borrow_mut();
                    r.failures += 1;
                    r.failures
                };
                next = MAX_BACKOFF_MS.max(poll_ms()).min(poll_ms() * 2u64.pow(failures.min(4)));
                let before = rt.borrow().state.clone();
                let replaced = self.mark_down(&rt, HostStatus::Error, &error);
                if before.status != HostStatus::Error || before.error.as_deref() != Some(error.as_str()) || replaced {
                    self.emit(RegistryEvent::Overview);
                }
            }
        }
        let mut r = rt.borrow_mut();
        r.polling = false;
        if r.stopped {
            return;
        }
        if r.poll_again {
            r.poll_again = false;
            next = 100;
        }
        drop(r);
        let t = Self::poll_later(&rt, next);
        rt.borrow_mut().timer = Some(t);
    }

    /// One poll's work; the error is the message the machine shows.
    async fn poll_once(&self, rt: &Rt, host: &Host) -> Result<(), String> {
        let pr = probe(host).await?;
        let pr = Rc::new(RefCell::new(pr));
        rt.borrow_mut().probe = Some(pr.clone());
        self.annotate_agents(rt, &pr).await.map_err(|e| e.message)?;
        rest_windows(rt, host, &pr.borrow());
        let config = host.config();
        let prev = rt.borrow().state.clone();
        let next = {
            let p = pr.borrow();
            let now = now_ms();
            HostState {
                config: config.clone(),
                status: HostStatus::Online,
                error: None,
                hostname: Some(p.hostname.clone()),
                integration: p.integration,
                agy_integration: Some(match p.agy_integration {
                    Some(true) => IntegrationState::Installed,
                    Some(false) => IntegrationState::Missing,
                    None => IntegrationState::Unknown,
                }),
                tmux: match p.tmux {
                    TmuxProbe::Ok => TmuxState::Ok,
                    TmuxProbe::NoServer => TmuxState::NoServer,
                    TmuxProbe::Missing => TmuxState::Missing,
                },
                agents: p.installed.clone(),
                signed_in: Some(p.signed_in.clone()),
                os: Some(p.os.clone()),
                machine_kind: p.machine_kind,
                node_version: if reached_through_inyon(&config) { node_link::info(&config.id).and_then(|i| i.version) } else { None },
                node_warning: if reached_through_inyon(&config) { node_link::info(&config.id).and_then(|i| i.warning) } else { None },
                node_update: None,
                latency_ms: Some(p.latency_ms as u64),
                sessions: p.sessions.clone(),
                updated_at: now,
                last_seen: Some(now),
                offline_since: None,
                limits: {
                    let r = rt.borrow();
                    limits_list(r.usage.as_ref(), p.account.as_deref(), r.codex.as_ref(), now)
                },
            }
        };
        {
            let mut r = rt.borrow_mut();
            r.state = next;
            r.failures = 0;
        }
        if let Some(since) = prev.offline_since {
            self.came_back(rt, since);
        }
        self.snapshot(rt, false);
        self.diff_notify(rt, &prev);
        let (integration, has_events) = (pr.borrow().integration, rt.borrow().events.is_some());
        if integration == IntegrationState::Installed && !has_events {
            self.watch_events(rt).map_err(|e| e.message)?;
        }
        let (r2, p2) = (rt.clone(), pr.clone());
        spawn(async move {
            if let Err(e) = registry().heal_integration(&r2, &p2).await {
                console::error(&format!("integration heal: {}", e.message));
            }
        });
        // live updates for every tmux session that has an agent in it
        let with_agents: Vec<String> = pr.borrow().sessions.iter().filter(|s| s.windows.iter().any(|w| w.panes.iter().any(|p| p.agent.is_some()))).map(|s| s.id.clone()).collect();
        let live = rt.borrow().live.clone();
        if let Some(l) = live {
            l.sync(&with_agents);
        }
        let mut prev = prev;
        prev.config = rt.borrow().state.config.clone();
        if overview_changed(&prev, &rt.borrow().state) {
            self.emit(RegistryEvent::Overview);
        }
        Ok(())
    }

    /// The machine isn't answering. The first time, its sessions become "last seen" (nothing is known
    /// to be running) and the outage starts; after a minute you're told once. True when the state
    /// was replaced.
    pub(crate) fn mark_down(&self, rt: &Rt, status: HostStatus, error: &str) -> bool {
        let st = rt.borrow().state.clone();
        let now = now_ms();
        let offline_since = st.offline_since.or(if st.status == HostStatus::Online || st.last_seen.is_some_and(|l| l != 0) { Some(now) } else { None });
        if st.status == status && st.error.as_deref() == Some(error) && st.offline_since == offline_since {
            self.maybe_notify_offline(rt);
            return false;
        }
        let online = st.status == HostStatus::Online;
        {
            let mut r = rt.borrow_mut();
            if online {
                // shown as unknown while away, but a turn that finishes meanwhile still counts as done, and
                // a question that was already waiting isn't announced again
                r.before_outage = Some(panes(&st.sessions).filter_map(|p| p.agent.as_ref().map(|a| (p.id.clone(), (a.kind, a.state)))).collect());
            }
            let sessions = if online { offline_sessions(&st.sessions) } else { st.sessions.clone() };
            r.state = HostState { status, error: Some(error.to_owned()), latency_ms: None, offline_since, sessions, ..st };
        }
        if online {
            self.snapshot(rt, true);
        }
        self.maybe_notify_offline(rt);
        true
    }

    fn maybe_notify_offline(&self, rt: &Rt) {
        let (since, notified, id, name) = {
            let r = rt.borrow();
            (r.state.offline_since, r.offline_notified, r.host.id(), r.host.config().name)
        };
        let due =
            offline_alert_due(OfflineAlert { since, notified, now: now_ms(), held: self.alerts_held.get(), held_until: self.alerts_from.get(), online: ONLINE.with(Cell::get) });
        if !due {
            return;
        }
        rt.borrow_mut().offline_notified = true;
        self.emit(RegistryEvent::Notify {
            host: id,
            pane: String::new(),
            kind: NotifyKind::Offline,
            title: format!("{name} is offline"),
            body: "Its sessions show as they were last seen. Inyon reconnects by itself.".into(),
            agent: None,
        });
    }

    fn came_back(&self, rt: &Rt, since: u64) {
        let (notified, id, name) = {
            let r = rt.borrow();
            (r.offline_notified, r.host.id(), r.host.config().name)
        };
        if notified {
            let mins = crate::js::js_round((now_ms() as f64 - since as f64) / 60_000.0).max(1.0);
            self.emit(RegistryEvent::Notify {
                host: id,
                pane: String::new(),
                kind: NotifyKind::Online,
                title: format!("{name} is back online"),
                body: format!("Reconnected after {} min. Sessions are up to date.", crate::js::fmt_number(mins)),
                agent: None,
            });
        }
        rt.borrow_mut().offline_notified = false;
    }

    /// Saves every machine's latest look that the 15 s limit held back (before the app may close).
    pub fn save_all(&self) {
        let all: Vec<Rt> = self.hosts.borrow().values().cloned().collect();
        for rt in all {
            let due = {
                let r = rt.borrow();
                r.state.status == HostStatus::Online && Some(to_json(&r.state.sessions)) != r.saved_snapshot
            };
            if due {
                self.snapshot(&rt, true);
            }
        }
    }

    /// Saves what the machine looks like now; skipped when nothing changed, at most every 15 s.
    fn snapshot(&self, rt: &Rt, force: bool) {
        let mut r = rt.borrow_mut();
        let key = to_json(&r.state.sessions);
        if !force && (Some(&key) == r.saved_snapshot.as_ref() || now_ms().saturating_sub(r.saved_at.unwrap_or(0)) < 15_000) {
            return;
        }
        r.saved_snapshot = Some(key);
        r.saved_at = Some(now_ms());
        let mut state = r.state.clone();
        state.config = r.host.config();
        drop(r);
        save_snapshot(&state);
    }

    /// Adds the agent, its session mapping, live status and on-screen state to each agent pane.
    pub(crate) async fn annotate_agents(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>) -> HubResult<()> {
        let host = rt.borrow().host.clone();
        let agent_panes: Vec<PaneInfo> = {
            let p = pr.borrow();
            panes(&p.sessions).filter(|x| p.agent_procs.contains_key(&x.id)).cloned().collect()
        };
        let ids: Vec<String> = agent_panes.iter().map(|p| p.id.clone()).collect();
        let screens = Rc::new(capture_panes(&host, &ids).await?);
        let live: Rc<RefCell<HashSet<i64>>> = Rc::default();
        // every Codex on the host, so two in one folder don't claim the same thread
        let codex_procs: Rc<Vec<CodexProc>> = Rc::new(
            pr.borrow()
                .agent_procs
                .values()
                .filter(|x| x.kind == AgentKind::Codex)
                .map(|x| CodexProc { pid: x.pid as i64, cwd: x.cwd.clone(), started_at: x.started_at, resume: x.resume.clone(), title: None })
                .collect(),
        );
        let work = agent_panes.into_iter().map(|p| self.annotate_pane(rt, &host, pr, p, screens.clone(), codex_procs.clone(), live.clone()));
        let done: Vec<HubResult<PaneInfo>> = join_all(work).await;
        let mut first_error = None;
        {
            let mut p = pr.borrow_mut();
            for r in done {
                match r {
                    Ok(pane) => {
                        if let Some(slot) = panes_mut(&mut p.sessions).find(|x| x.id == pane.id) {
                            *slot = pane;
                        }
                    }
                    Err(e) => {
                        first_error.get_or_insert(e);
                    }
                }
            }
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        crate::features::companion::mark(&host.id(), &mut pr.borrow_mut().sessions, crate::features::companion::agent().is_some());
        let agent_ids: Vec<String> = {
            let p = pr.borrow();
            panes(&p.sessions).filter(|x| p.agent_procs.contains_key(&x.id)).map(|x| x.id.clone()).collect()
        };
        self.check_reporting(rt, pr, &agent_ids).await?;
        self.apply_usage(rt, pr, &agent_ids);
        self.ask_codex_limits(rt, pr);
        let live = live.borrow();
        let mut r = rt.borrow_mut();
        r.cwd_map.retain(|pid, _| live.contains(pid));
        r.status_cache.retain(|pid, _| live.contains(pid));
        r.screens.retain(|id, _| agent_ids.contains(id));
        Ok(())
    }

    /// One agent pane of a poll; returns the pane as annotated.
    #[allow(clippy::too_many_arguments)]
    async fn annotate_pane(
        &self,
        rt: &Rt,
        host: &Host,
        pr: &Rc<RefCell<ProbeResult>>,
        mut p: PaneInfo,
        screens: Rc<IndexMap<String, Vec<String>>>,
        codex_procs: Rc<Vec<CodexProc>>,
        live: Rc<RefCell<HashSet<i64>>>,
    ) -> HubResult<PaneInfo> {
        let host_id = host.id();
        let (proc, hook, home, agy_conversation, parked) = {
            let pr = pr.borrow();
            let proc = pr.agent_procs.get(&p.id).cloned().expect("an agent pane");
            let conv = pr.agy_conversations.get(&(proc.pid as i64)).cloned();
            let parked = pr.claude_parked.get(&(proc.pid as i64)).cloned().filter(|_| proc.kind == AgentKind::Claude);
            let kind = proc.kind;
            (proc, pr.hook_panes.get(&p.id).filter(|h| h.from_agent(kind)).cloned(), Some(pr.home.clone()).filter(|h| !h.is_empty()), conv, parked)
        };
        let pid = proc.pid as i64;
        live.borrow_mut().insert(pid);
        let mut info = AgentPaneInfo {
            kind: proc.kind,
            custom: proc.custom.clone(),
            pid: proc.pid as u32,
            session_id: None,
            transcript_path: None,
            mapped_by: None,
            state: AgentState::Unknown,
            mode: None,
            waiting_for: None,
            options: None,
            activity: None,
            background: None,
            ultracode: None,
            status: None,
            status_stopped: None,
            started_at: Some(crate::js::js_round(proc.started_at * 1000.0) as u64),
            failure: None,
        };
        let hook_path = hook.as_ref().and_then(|h| Some(&h.data).at("transcript_path").filter(|v| Some(*v).truthy()).map(json::value_string));
        if let Some(b) = parked {
            // the window shows the background session it moved its conversation to, and types into it
            info.transcript_path = Some(b.transcript);
            info.session_id = Some(b.session_id);
            info.mapped_by = Some(MappedBy::Process);
        } else if proc.kind == AgentKind::Claude && hook_path.is_some() && hook.as_ref().is_some_and(|h| h.ts >= proc.started_at - 5.0) {
            info.transcript_path = hook_path;
            info.session_id = hook.as_ref().and_then(|h| Some(&h.data).at("session_id").filter(|v| !v.is_null()).map(json::value_string));
            info.mapped_by = Some(MappedBy::Hook);
        } else if proc.kind == AgentKind::Antigravity {
            // the conversation whose presence lock the process holds: exact, no guessing
            if let (Some(id), Some(home)) = (agy_conversation.filter(|c| !c.is_empty()), home.as_deref()) {
                info.transcript_path = Some(agy_transcript(home, &id));
                info.session_id = Some(id);
                info.mapped_by = Some(MappedBy::Process);
            }
        } else if let Some(home) = home.as_deref().filter(|_| proc.kind.chat()) {
            // a terminal-first agent has no transcript Inyon reads, so none is looked for
            let cached = rt.borrow().cwd_map.get(&pid).cloned();
            // re-check often until the session's first transcript appears, then occasionally (for /clear)
            // an OpenCode TUI that hasn't started a conversation yet ("opencode-db:") is checked often too
            // OpenCode and Codex put the conversation's name in the terminal title, so a change there is
            // a switch (/new, /resume), found at once rather than at the next 15 s check
            let stale = match &cached {
                None => true,
                Some(c) => {
                    let keep = if c.path.as_deref().is_some_and(|x| !x.is_empty() && x != OCDB) { 15000 } else { 2000 };
                    now_ms().saturating_sub(c.at) > keep || (matches!(proc.kind, AgentKind::Opencode | AgentKind::Codex) && c.title != p.title)
                }
            };
            let cached = if stale {
                let path = match proc.kind {
                    AgentKind::Claude => {
                        find_transcript(host, home, &TranscriptProc { cwd: proc.cwd.as_deref(), started_at: proc.started_at, resume: proc.resume.as_deref() }).await?
                    }
                    AgentKind::Codex => {
                        let me = CodexProc { pid, cwd: proc.cwd.clone(), started_at: proc.started_at, resume: proc.resume.clone(), title: Some(p.title.clone()) };
                        find_codex_rollout(host, home, &me, &codex_procs).await?
                    }
                    AgentKind::Gemini => find_gemini_session(host, home, &GeminiProc { cwd: proc.cwd.clone(), started_at: proc.started_at }).await?,
                    AgentKind::Copilot => find_copilot_session(host, home, &CopilotProc { pid, resume: proc.resume.clone() }).await?,
                    // its local server when started with --port, else its database
                    _ => match opencode_endpoint(host, pid, proc.cwd.as_deref(), Some(&p.title)).await? {
                        Some(uri) => Some(uri),
                        None => {
                            let oc = OpenCodeProc { cwd: proc.cwd.clone(), started_at: proc.started_at, args: Some(proc.args.clone()) };
                            find_open_code_session(host, &oc, screens.get(&p.id).map(Vec::as_slice), Some(&p.title)).await?
                        }
                    },
                };
                let c = Mapped { path, at: now_ms(), title: p.title.clone() };
                rt.borrow_mut().cwd_map.insert(pid, c.clone());
                c
            } else {
                cached.expect("a cached mapping")
            };
            if let Some(path) = cached.path.filter(|x| !x.is_empty()) {
                // a Codex segment file after a rewind is "<thread>_<segment>"; the thread is the session
                let last = path.rsplit('/').next().unwrap_or("");
                let id = re!(r"\.jsonl?$").replacen(last, 1, "");
                let id = re!(r"^(rollout-[\dT-]+-|session-)").replacen(&id, 1, "");
                let id = re!(r"_[0-9a-f-]{36}$").replacen(&id, 1, "").into_owned();
                info.session_id = Some(id);
                if proc.kind == AgentKind::Copilot {
                    let parts: Vec<&str> = path.split('/').collect();
                    info.session_id = parts.len().checked_sub(2).map(|i| parts[i].to_owned());
                }
                if proc.kind == AgentKind::Opencode {
                    info.session_id = if path.starts_with(OCDB) { ocdb_session(&path).map(str::to_owned) } else { Some(last.to_owned()).filter(|s| !s.is_empty()) };
                }
                info.transcript_path = Some(path);
                info.mapped_by = Some(if proc.resume.as_deref().is_some_and(|r| !r.is_empty()) { MappedBy::Resume } else { MappedBy::Cwd });
            }
        }
        // live numbers
        if proc.kind == AgentKind::Claude {
            // a session's last word (its Stop at the end of a turn, say) is activity, whatever tmux
            // noticed; not the "waiting for your input" notice a minute after it went quiet
            let said = live_hook(&pr.borrow(), &p.id);
            if let Some(said) = said {
                if Some(&said.data).at("hook_event_name").str() != Some("Notification") {
                    was_active(&mut pr.borrow_mut(), &p.id, said.ts);
                }
            }
            info.status = {
                let pr = pr.borrow();
                let session = if info.mapped_by == Some(MappedBy::Hook) { info.session_id.as_deref() } else { None };
                claude_status(pr.integration, &pr.status_panes, &pr.compact_panes, &p.id, proc.started_at, session)
            };
            // Claude rewrites its status as it redraws (after /model, say): a moment without a model
            // isn't a change of model, so keep the last one for this conversation
            let was = self.state_agent(rt, &p.id);
            let same = was.as_ref().is_some_and(|w| w.session_id == info.session_id);
            let was_status = was.as_ref().and_then(|w| w.status.clone());
            match (&mut info.status, was_status) {
                (None, Some(ws)) if same => info.status = Some(ws),
                (Some(st), Some(ws)) if st.model.is_none() && same && ws.model.is_some() => {
                    st.model = ws.model;
                    st.model_id = ws.model_id;
                    if st.effort.is_none() {
                        st.effort = ws.effort;
                    }
                }
                _ => {}
            }
        }
        if proc.kind == AgentKind::Codex {
            if let Some(path) = info.transcript_path.clone() {
                let cached = rt.borrow().status_cache.get(&pid).cloned().filter(|c| now_ms().saturating_sub(c.at) <= 8000);
                let c = match cached {
                    Some(c) => c,
                    None => {
                        let s = read_codex_status(host, &path).await?;
                        let c = CachedStatus { status: s.status, permission: s.permission, at: now_ms() };
                        rt.borrow_mut().status_cache.insert(pid, c.clone());
                        c
                    }
                };
                info.status = c.status;
                // the permission preset isn't on screen; Plan mode (on screen) overrides it below
                if let Some(perm) = c.permission.filter(|x| !x.is_empty()) {
                    info.mode = Some(perm);
                }
            }
            p.title = codex_title(&p.title, Some(proc.cwd.as_deref().unwrap_or(&p.cwd)));
        }
        // OpenCode titles the terminal "OC | <conversation>", or "OpenCode" before there is one
        if proc.kind == AgentKind::Opencode {
            let t = re!(r"^OC \| ").replacen(&p.title, 1, "").into_owned();
            p.title = re!(r"^OpenCode$").replacen(&t, 1, "").into_owned();
        }
        if proc.kind == AgentKind::Antigravity {
            let (st, title, generic) = {
                let pr = pr.borrow();
                let st = agy_status(&pr.agy_status, pr.agy_integration, &p.id, proc.started_at, info.session_id.as_deref());
                // agy leaves the pane title alone (the host name); show the conversation's title instead
                let title = info
                    .session_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .and_then(|s| pr.agy_titles.get(s).cloned())
                    .filter(|t| !t.is_empty())
                    .or_else(|| st.as_ref().and_then(|s| s.title.clone()));
                // the shell may have set it to the command line ("agy --model … -i '…'"); that's no name either
                let generic = p.title.is_empty() || p.title == pr.hostname || re!(r"^(agy|antigravity)(\s|$)", "i").is_match(&p.title);
                (st, title, generic)
            };
            if let Some(st) = st {
                info.status = Some(st.status);
            }
            match title.filter(|t| !t.is_empty()) {
                Some(t) if generic => p.title = t,
                _ if generic => p.title = String::new(),
                _ => {}
            }
        }
        // the pane as it was last seen, for what the screen doesn't say this time
        let was = self.state_agent(rt, &p.id);
        let mode = info.mode.clone();
        p.agent = Some(info);
        if let Some(lines) = screens.get(&p.id) {
            let sc = parse_screen(lines, width_of(&p), proc.kind);
            rt.borrow_mut().screens.insert(p.id.clone(), sc.clone());
            let hook = live_hook(&pr.borrow(), &p.id);
            apply_screen(&mut p, sc.clone(), host, lines, hook.as_ref(), was.as_ref(), mode);
            p = apply_open_code_live(host, p, Some(&sc)).await;
        }
        if proc.kind == AgentKind::Claude {
            self.apply_failure(rt, pr, &mut p);
        }
        let info = p.agent.clone().expect("the agent");
        // a rename the agent hasn't taken yet: done once its title says so, else typed when it's idle
        if let Some(want) = pending_name(&host_id, info.session_id.as_deref()) {
            if owns_title(Some(proc.kind)) {
                if bare_title(Some(&p.title)) == want.name {
                    name_taken(&host_id, info.session_id.as_deref().unwrap_or(""));
                } else if info.state == AgentState::Idle {
                    self.emit(RegistryEvent::RenameDue { host: host_id.clone(), pane: p.id.clone() });
                }
            }
        }
        p.label = session_name(&host_id, info.session_id.as_deref(), Some(proc.kind));
        // agy's own word on its mode, for when a custom status line hides the footer
        if proc.kind == AgentKind::Antigravity && info.mode.is_none() {
            let m = {
                let pr = pr.borrow();
                agy_status(&pr.agy_status, pr.agy_integration, &p.id, proc.started_at, info.session_id.as_deref()).and_then(|s| s.mode)
            };
            if let (Some(m), Some(a)) = (m.filter(|m| !m.is_empty()), p.agent.as_mut()) {
                a.mode = Some(m);
            }
        }
        Ok(p)
    }

    /// A turn that ended on an error (the pane's last hook is a StopFailure) says why, and when a
    /// full rate limit resets: from the session's own numbers and the account's.
    fn apply_failure(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>, p: &mut PaneInfo) {
        let hook = live_hook(&pr.borrow(), &p.id);
        // the account's limits only as apply_usage takes them: this account's, and fresh
        let signed_in = pr.borrow().account.clone();
        let account = rt
            .borrow()
            .usage
            .as_ref()
            .filter(|u| u.account == signed_in && u.ok_at.is_some_and(|o| o != 0 && now_ms().saturating_sub(o) <= USAGE_STALE_MS))
            .map(|u| (u.u.five_hour.clone(), u.u.seven_day.clone()));
        let Some(a) = p.agent.as_mut() else { return };
        a.failure = hook.filter(|_| a.state != AgentState::Working).and_then(|h| {
            let own = a.status.as_ref().map(|s| (s.five_hour.clone(), s.seven_day.clone()));
            let windows: Vec<RateWindow> = [own, account].into_iter().flatten().flat_map(|(f, s)| [f, s]).flatten().collect();
            turn_failure(&h, &windows.iter().collect::<Vec<_>>(), now_ms() as f64 / 1000.0)
        });
    }

    /// The agent the machine's current state shows in a pane (`findPane(..)?.pane.agent`).
    fn state_agent(&self, rt: &Rt, pane_id: &str) -> Option<AgentPaneInfo> {
        let r = rt.borrow();
        let found = panes(&r.state.sessions).find(|p| p.id == pane_id).and_then(|p| p.agent.clone());
        found
    }

    /// Limits are per account, so they come from the account (like /usage), not from whatever a
    /// session's status line last said: right after a /login or a credentials swap, with no restart.
    fn apply_usage(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>, pane_ids: &[String]) {
        let (installed, account) = {
            let p = pr.borrow();
            (p.installed.contains(&AgentKind::Claude), p.account.clone())
        };
        if !installed {
            return;
        }
        let (u, loading, host) = {
            let r = rt.borrow();
            (r.usage.clone(), r.usage_loading, r.host.clone())
        };
        let due = match &u {
            None => true,
            Some(u) => now_ms().saturating_sub(u.u.at) > (if u.u.error.is_some() { USAGE_RETRY_MS } else { USAGE_EVERY_MS }) || u.account != account,
        };
        if due && !loading {
            rt.borrow_mut().usage_loading = true;
            let (rt2, account2) = (rt.clone(), account.clone());
            spawn(async move {
                match fetch_claude_usage(&host).await {
                    Ok(r) => {
                        let id = host.id();
                        {
                            let mut me = rt2.borrow_mut();
                            let prev = me.usage.clone().filter(|p| p.account == account2);
                            me.usage = Some(match &r.error {
                                Some(e) => {
                                    let mut keep = prev.unwrap_or_default();
                                    keep.u.at = r.at;
                                    keep.u.error = Some(e.clone());
                                    keep.account = account2.clone();
                                    keep
                                }
                                None => Usage { ok_at: Some(r.at), u: r.clone(), account: account2.clone() },
                            });
                        }
                        if let Some(e) = &r.error {
                            console::error(&format!("usage {id}: {e}"));
                        }
                        registry().refresh(&id, 0);
                    }
                    Err(e) => console::error(&format!("usage: {}", e.message)),
                }
                rt2.borrow_mut().usage_loading = false;
            });
        }
        let Some(u) = u else { return };
        let Some(ok_at) = u.ok_at.filter(|o| *o != 0) else { return };
        if u.account != account || now_ms().saturating_sub(ok_at) > USAGE_STALE_MS {
            return;
        }
        let mut p = pr.borrow_mut();
        for pane in panes_mut(&mut p.sessions).filter(|x| pane_ids.contains(&x.id)) {
            let Some(a) = pane.agent.as_mut().filter(|a| a.kind == AgentKind::Claude) else { continue };
            let mut st = a.status.clone().unwrap_or_else(|| crate::agents::status::empty_status(0.0));
            st.five_hour = u.u.five_hour.clone();
            st.seven_day = u.u.seven_day.clone();
            if let Some(plan) = u.u.plan.clone() {
                st.plan = Some(plan);
            }
            if let Some(stale) = st.stale.as_mut().filter(|s| s.limits.is_some()) {
                stale.limits = None;
            }
            a.status = Some(st);
        }
    }

    /// Asks Codex for its limits on a machine where it's installed and signed in, every few minutes
    /// while someone's looking.
    fn ask_codex_limits(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>) {
        let ready = {
            let p = pr.borrow();
            p.installed.contains(&AgentKind::Codex) && p.signed_in.contains(&AgentKind::Codex)
        };
        if !ready {
            rt.borrow_mut().codex = None;
            return;
        }
        let watched = POLL.with(|p| p.get()) != QUIET_POLL_MS;
        let (due, host) = {
            let r = rt.borrow();
            (!r.codex_loading && r.codex.as_ref().is_none_or(|c| now_ms().saturating_sub(c.at) > CODEX_EVERY_MS), r.host.clone())
        };
        if !due || !watched {
            return;
        }
        rt.borrow_mut().codex_loading = true;
        let rt2 = rt.clone();
        spawn(async move {
            let got = match codex_limits::fetch(&host).await {
                Ok(r) => r,
                Err(e) => Err(e.message),
            };
            let id = host.id();
            {
                let mut me = rt2.borrow_mut();
                let kept = me.codex.as_ref().and_then(|c| c.limits.clone());
                me.codex = Some(match got {
                    Ok(limits) => CodexLimits { limits, at: now_ms() },
                    Err(e) => {
                        console::error(&format!("codex limits {id}: {e}"));
                        CodexLimits { limits: kept, at: now_ms() }
                    }
                });
                me.codex_loading = false;
            }
            registry().refresh(&id, 0);
        });
    }

    /// A Claude session writes its status line after every message. If its transcript has moved on
    /// for a while and the status hasn't, the session stopped reporting (its settings were swapped
    /// under it), and whatever it last wrote is out of date.
    async fn check_reporting(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>, pane_ids: &[String]) -> HubResult<()> {
        let claude: Vec<(String, String, Option<u64>)> = {
            let p = pr.borrow();
            if p.integration != IntegrationState::Installed {
                return Ok(());
            }
            panes(&p.sessions)
                .filter(|x| pane_ids.contains(&x.id))
                .filter_map(|x| {
                    x.agent
                        .as_ref()
                        .filter(|a| a.kind == AgentKind::Claude)
                        .and_then(|a| a.transcript_path.clone().filter(|t| !t.is_empty()).map(|t| (x.id.clone(), t, a.started_at)))
                })
                .collect()
        };
        if claude.is_empty() {
            return Ok(());
        }
        let mut paths: Vec<&str> = vec![];
        for (_, t, _) in &claude {
            if !paths.contains(&t.as_str()) {
                paths.push(t);
            }
        }
        // The newest entry's own timestamp, not the file's mtime: Claude also touches the file
        // without adding a message, which would look like a session that stopped reporting.
        let host = rt.borrow().host.clone();
        let r = host
            .run(&format!(
                r#"for f in {}; do echo "$(tail -c 262144 "$f" 2>/dev/null | grep -o '"timestamp":"[^"]*"' | tail -n 1 | cut -d'"' -f4) $f"; done"#,
                paths.iter().map(|x| shq(x)).collect::<Vec<_>>().join(" ")
            ))
            .await?;
        let mut last: HashMap<String, f64> = HashMap::new();
        for line in r.stdout.split('\n') {
            if let Some(m) = re!(r"^(\S+) (.+)$").captures(line) {
                let t = date::parse(&m[1]);
                if !t.is_nan() {
                    last.insert(m[2].to_owned(), t / 1000.0);
                }
            }
        }
        {
            let mut p = pr.borrow_mut();
            for (id, path, started_at) in &claude {
                let moved = last.get(path).copied().unwrap_or(0.0);
                was_active(&mut p, id, moved);
                let since = p.status_panes.get(id).map_or(0.0, |s| s.ts).max(started_at.unwrap_or(0) as f64 / 1000.0);
                if moved > since + STALL_S {
                    if let Some(a) = panes_mut(&mut p.sessions).find(|x| x.id == *id).and_then(|x| x.agent.as_mut()) {
                        // keep nothing it reported: limits come from the account and context from the transcript
                        a.status = None;
                        a.status_stopped = Some(true);
                    }
                }
            }
        }
        let stalled: Vec<PaneInfo> = {
            let p = pr.borrow();
            panes(&p.sessions).filter(|x| claude.iter().any(|(id, _, _)| *id == x.id)).cloned().collect()
        };
        let (rt2, pr2) = (rt.clone(), pr.clone());
        spawn(async move {
            if let Err(e) = registry().auto_heal(&rt2, &pr2, stalled).await {
                console::error(&format!("auto-heal: {}", e.message));
            }
        });
        Ok(())
    }

    /// Puts a replaced integration back, and resumes idle sessions that stopped reporting.
    async fn auto_heal(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>, claude: Vec<PaneInfo>) -> HubResult<()> {
        let host = rt.borrow().host.clone();
        let id = host.id();
        if host.config().keep_integration != Some(true) {
            return Ok(());
        }
        for p in claude {
            let a = p.agent.clone().expect("a Claude pane");
            let Some(session) = a.session_id.clone().filter(|s| !s.is_empty()) else { continue };
            if a.status_stopped != Some(true) {
                continue;
            }
            // first, without touching the session: point its project settings at the status line
            let reconnected = rt.borrow().reconnected.get(&session).copied();
            let Some(reconnected) = reconnected else {
                {
                    let mut r = rt.borrow_mut();
                    // a hub that runs for months sees many sessions: forget ones from days ago
                    if r.reconnected.len() > 500 {
                        let old: Vec<String> = r.reconnected.iter().filter(|(_, at)| now_ms().saturating_sub(**at) > 86_400_000).map(|(k, _)| k.clone()).collect();
                        for k in old {
                            r.reconnected.shift_remove(&k);
                            r.resumed.remove(&k);
                        }
                    }
                    r.reconnected.insert(session.clone(), now_ms());
                }
                console::log(&format!("reconnect {id} {} {session}: stopped reporting live numbers", p.id));
                if let Err(e) = crate::claude::actions::reconnect_agent(&id, &p.id).await {
                    console::error(&format!("reconnect {}: {}", p.id, e.message));
                }
                continue;
            };
            // still silent a few minutes later: resuming is the last resort, and only if you allowed it
            if now_ms().saturating_sub(reconnected) < 3 * 60_000 || rt.borrow().resumed.contains(&session) {
                continue;
            }
            // resuming by a guessed session id could open the wrong conversation
            if a.mapped_by != Some(MappedBy::Hook) {
                continue;
            }
            if load_settings().auto_resume() != Some(true) {
                continue;
            }
            let sc = rt.borrow().screens.get(&p.id).cloned();
            let activity = pr.borrow().sessions.iter().flat_map(|s| s.windows.iter()).find(|w| w.panes.iter().any(|x| x.id == p.id)).map_or(0.0, |w| *w.activity);
            let quiet_for = now_ms() as f64 / 1000.0 - activity;
            // only when nothing would be lost: idle, nothing typed, no background shells, quiet a while
            let Some(sc) = sc else { continue };
            if sc.working
                || sc.prompt.is_some()
                || sc.draft.as_deref().is_some_and(|d| !d.is_empty())
                || sc.background.clone().flatten().is_some_and(|b| !b.is_empty())
                || a.state != AgentState::Idle
                || quiet_for < 45.0
            {
                continue;
            }
            rt.borrow_mut().resumed.insert(session.clone());
            console::log(&format!("auto-resume {id} {} {session}: stopped reporting live numbers", p.id));
            crate::claude::actions::restart_agent(&id, &p.id).await?;
        }
        Ok(())
    }

    /// Called after each probe: the integration went missing from settings, so add it back.
    async fn heal_integration(&self, rt: &Rt, pr: &Rc<RefCell<ProbeResult>>) -> HubResult<()> {
        let host = rt.borrow().host.clone();
        let cfg = host.config();
        let (integration, agy, installed, hooked) = {
            let p = pr.borrow();
            (p.integration, p.agy_integration, p.installed.clone(), p.hooked.clone())
        };
        if integration == IntegrationState::Installed && cfg.keep_integration.is_none() {
            self.set_keep_integration(&cfg.id, true)?;
            return Ok(());
        }
        // an agent with hooks turned up after the integration was installed (or its settings were
        // replaced): add Inyon's hook to it
        let missing = HOOKED_AGENTS.iter().any(|k| installed.contains(k) && !hooked.contains(k));
        if cfg.keep_integration == Some(true) && integration == IntegrationState::Installed && missing {
            let healed = rt.borrow().hooks_healed_at;
            if healed.is_none_or(|h| h == 0 || now_ms().saturating_sub(h) >= HEAL_EVERY_MS) {
                rt.borrow_mut().hooks_healed_at = Some(now_ms());
                console::log(&format!("agent hooks missing on {}: adding them", cfg.id));
                install_agent_hooks(&host).await?;
                self.refresh(&cfg.id, 500);
                return Ok(());
            }
        }
        // agy turned up after the integration was installed (or its settings were replaced): add its status line
        if cfg.keep_integration == Some(true) && integration == IntegrationState::Installed && agy == Some(false) && installed.contains(&AgentKind::Antigravity) {
            let healed = rt.borrow().agy_healed_at;
            if healed.is_some_and(|h| h != 0 && now_ms().saturating_sub(h) < HEAL_EVERY_MS) {
                return Ok(());
            }
            rt.borrow_mut().agy_healed_at = Some(now_ms());
            console::log(&format!("antigravity integration missing on {}: adding it", cfg.id));
            crate::claude::integration::install_agy_integration(&host).await?;
            self.refresh(&cfg.id, 500);
            return Ok(());
        }
        if cfg.keep_integration != Some(true) || !installed.contains(&AgentKind::Claude) {
            return Ok(());
        }
        if integration != IntegrationState::Removed && integration != IntegrationState::Outdated {
            return Ok(());
        }
        let healed = rt.borrow().healed_at;
        if healed.is_some_and(|h| h != 0 && now_ms().saturating_sub(h) < HEAL_EVERY_MS) {
            return Ok(());
        }
        rt.borrow_mut().healed_at = Some(now_ms());
        let word = if integration == IntegrationState::Removed { "removed" } else { "outdated" };
        console::log(&format!("integration {word} on {}: putting it back", cfg.id));
        crate::claude::integration::install_integration(&host).await?;
        self.refresh(&cfg.id, 500);
        Ok(())
    }

    pub(crate) fn diff_notify(&self, rt: &Rt, prev: &HostState) {
        let before: HashMap<&str, &PaneInfo> = panes(&prev.sessions).map(|p| (p.id.as_str(), p)).collect();
        let (state, seen, host_id) = {
            let mut r = rt.borrow_mut();
            let seen = if r.state.status == HostStatus::Online { r.before_outage.take() } else { None };
            (r.state.clone(), seen, r.host.id())
        };
        for s in &state.sessions {
            for w in &s.windows {
                for p in &w.panes {
                    let mut was = before.get(p.id.as_str()).and_then(|b| b.agent.as_ref()).map(|a| a.state);
                    let last = seen.as_ref().and_then(|m| m.get(&p.id));
                    if let (Some(AgentState::Unknown), Some((kind, st))) = (was, last) {
                        if Some(*kind) == p.agent.as_ref().map(|a| a.kind) {
                            was = Some(*st);
                        }
                    }
                    let now = p.agent.as_ref().map(|a| a.state);
                    let agent_name = p.agent.as_ref().map_or("Agent".to_owned(), |a| agents()[&a.kind].name.clone());
                    let (Some(was), Some(now)) = (was, now) else { continue };
                    // a turn that ends with agents or shells still running isn't finished: the alert waits
                    // until the pane is idle with nothing left in the background
                    let had_background = before.get(p.id.as_str()).and_then(|b| b.agent.as_ref()).and_then(|a| a.background.as_deref()).is_some_and(|b| !b.is_empty());
                    let background = p.agent.as_ref().and_then(|a| a.background.as_deref()).is_some_and(|b| !b.is_empty());
                    let title = Some(pane_title(p)).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{}:{}", s.name, w.index));
                    let key = turn_key(&host_id, &p.id);
                    // anything but a bare idle pane means the turn goes on: a finish on hold is off
                    if now != AgentState::Idle || background {
                        if let Some(t) = self.done_timers.borrow_mut().remove(&key) {
                            t.clear();
                        }
                    }
                    let kind = p.agent.as_ref().map(|a| a.kind);
                    let done = {
                        let (host, pane, title, rt) = (host_id.clone(), p.id.clone(), title.clone(), rt.clone());
                        let failure = p.agent.as_ref().and_then(|a| a.failure.as_ref()).map(|f| failure_text(f, now_ms() as f64 / 1000.0));
                        let body = match &failure {
                            Some(f) => format!("{agent_name} stopped: {f}"),
                            None => format!("{agent_name} finished"),
                        };
                        let companion = p.companion == Some(true);
                        let stopped = failure.is_some().then(|| body.clone());
                        move || {
                            // its reply goes to wherever you talk to it, typed in the terminal or not
                            if companion {
                                return registry().emit(RegistryEvent::CompanionDone { host, pane, failure: stopped });
                            }
                            // they're at that terminal and saw it finish
                            if typed_lately(&rt, &pane) {
                                return;
                            }
                            registry().emit(RegistryEvent::Notify { host, pane, kind: NotifyKind::Done, title, body, agent: kind })
                        }
                    };
                    if was == now {
                        if now == AgentState::Idle && had_background && !background {
                            // the last agent or shell is gone: finished, unless the main turn picks up again
                            let pid = p.agent.as_ref().map(|a| a.pid);
                            if let Some(t) = self.done_timers.borrow_mut().remove(&key) {
                                t.clear();
                            }
                            let (rt2, k2, pane) = (rt.clone(), key.clone(), p.id.clone());
                            let timer = set_timeout(self.done_grace.get(), move || {
                                registry().done_timers.borrow_mut().remove(&k2);
                                let still = {
                                    let r = rt2.borrow();
                                    let still = panes(&r.state.sessions)
                                        .find(|x| x.id == pane)
                                        .and_then(|x| x.agent.as_ref())
                                        .is_some_and(|a| a.state == AgentState::Idle && a.background.as_deref().is_none_or(str::is_empty) && Some(a.pid) == pid);
                                    still
                                };
                                if still {
                                    done();
                                }
                            });
                            self.done_timers.borrow_mut().insert(key, timer);
                        }
                    } else if now == AgentState::Waiting {
                        let body = p.agent.as_ref().and_then(|a| a.waiting_for.clone()).unwrap_or_else(|| format!("{agent_name} needs your input"));
                        self.emit(RegistryEvent::Notify { host: host_id.clone(), pane: p.id.clone(), kind: NotifyKind::Waiting, title, body, agent: kind });
                    } else if was == AgentState::Working && now == AgentState::Idle && !background {
                        done();
                    }
                }
            }
        }
    }

    /// A pane printed something: re-read just the agent panes that changed, shortly.
    fn on_pane_output(&self, rt: &Rt, pane: &str) {
        let host_id = rt.borrow().host.id();
        self.emit(RegistryEvent::PaneOutput { host: host_id.clone(), pane: pane.to_owned() });
        if self.pane_agent(&host_id, pane).is_none() {
            return;
        }
        let mut r = rt.borrow_mut();
        r.dirty.insert(pane.to_owned());
        if r.dirty_timer.is_some() {
            return;
        }
        let rt2 = rt.clone();
        r.dirty_timer = Some(set_timeout(180, move || spawn(async move { registry().flush_dirty(&rt2).await })));
    }

    pub(crate) async fn flush_dirty(&self, rt: &Rt) {
        let (ids, host) = {
            let mut r = rt.borrow_mut();
            r.dirty_timer = None;
            let ids: Vec<String> = r.dirty.drain(..).collect();
            if ids.is_empty() || r.stopped || r.polling {
                return;
            }
            (ids, r.host.clone())
        };
        let host_id = host.id();
        let prev = rt.borrow().state.clone();
        let Ok(screens) = capture_panes(&host, &ids).await else { return };
        let mut changed = false;
        for id in &ids {
            let (Some(pane), Some(lines)) = (self.find_pane(&host_id, id).map(|f| f.pane), screens.get(id)) else { continue };
            let Some(agent) = pane.agent.clone() else { continue };
            let before = to_json(&agent);
            let sc = parse_screen(lines, width_of(&pane), agent.kind);
            rt.borrow_mut().screens.insert(id.clone(), sc.clone());
            let hook = rt.borrow().probe.as_ref().and_then(|p| live_hook(&p.borrow(), id));
            let base = self.off_screen_mode(rt, id, &agent);
            let mut p = pane;
            apply_screen(&mut p, sc.clone(), &host, lines, hook.as_ref(), Some(&agent), base);
            let p = apply_open_code_live(&host, p, Some(&sc)).await;
            if to_json(&p.agent) != before {
                changed = true;
            }
            // the TS changed the pane in place; here it's written back to whichever state now holds it
            self.update_pane(&host_id, id, |x| x.agent = p.agent);
        }
        if changed {
            rt.borrow_mut().state.updated_at = now_ms();
            self.diff_notify(rt, &prev);
            self.emit(RegistryEvent::Overview);
        }
    }

    /// A pane's mode as far as it's known without its screen, which is what the poll starts from:
    /// Codex's permission preset, Antigravity's own status line.
    fn off_screen_mode(&self, rt: &Rt, pane_id: &str, agent: &AgentPaneInfo) -> Option<String> {
        let r = rt.borrow();
        if agent.kind == AgentKind::Codex {
            return r.status_cache.get(&(agent.pid as i64)).and_then(|c| c.permission.clone());
        }
        let pr = r.probe.as_ref()?.borrow();
        let proc = pr.agent_procs.get(pane_id)?;
        if agent.kind == AgentKind::Antigravity {
            return agy_status(&pr.agy_status, pr.agy_integration, pane_id, proc.started_at, agent.session_id.as_deref()).and_then(|s| s.mode);
        }
        None
    }

    /// Follows the hook event log on the machine for instant updates.
    fn watch_events(&self, rt: &Rt) -> HubResult<()> {
        let host = rt.borrow().host.clone();
        let prepare = r#"umask 077; mkdir -p "$HOME/.inyon" && touch "$HOME/.inyon/events.jsonl""#;
        let tail = r#"tail -n0 -F "$HOME/.inyon/events.jsonl" 2>/dev/null"#;
        // on another machine it stops when the connection does (see until_stdin_closes)
        let remote = host.config().kind != HostKind::Local;
        let cmd = if remote { format!("{prepare} && {{ {}; }}", until_stdin_closes(tail)) } else { format!("{prepare} && exec {tail}") };
        let id = host.id();
        let s = host.stream(
            &cmd,
            move |line| {
                let Some(e) = json::parse(&line) else { return };
                let me = registry();
                me.refresh(&id, 120);
                // `e.pane` on null throws in the TS, after the refresh
                if e.is_null() {
                    return;
                }
                let e = Some(&e);
                if e.at("pane").truthy() {
                    let pane = json::value_string(e.at("pane").unwrap());
                    me.emit(RegistryEvent::Hook { host: id.clone(), pane, event: e.at("data").cloned().unwrap_or(Value::Null) });
                }
            },
            remote,
        )?;
        rt.borrow_mut().events = Some(s.clone());
        let rt2 = rt.clone();
        spawn(async move {
            s.exited.wait().await;
            let mut r = rt2.borrow_mut();
            if r.events.as_ref().is_some_and(|e| e.same(&s)) {
                r.events = None;
            }
        });
        Ok(())
    }

    /// For tests and the platform: a machine's runtime as it is, without polling it.
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    /// A test's machine, put in as it is (the TS tests spy on `registry.findPane`).
    pub fn insert_runtime(&self, id: &str, rt: Rt) {
        self.hosts.borrow_mut().insert(id.to_owned(), rt);
    }

    /// An event sent as the registry sends its own (the TS tests call its private `emit`).
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn emit_for_test(&self, e: RegistryEvent) {
        self.emit(e)
    }
}

/// A pane's width as the screen reader takes it.
pub(crate) fn width_of(p: &PaneInfo) -> Option<u32> {
    let w = *p.width;
    (w.is_finite() && w >= 0.0).then_some(w as u32)
}

/// Moves a window's last activity up to `at` (seconds): tmux's own only counts output, and psmux
/// doesn't keep it at all, so "Active 11m ago" counted from when the session started.
fn was_active(pr: &mut ProbeResult, pane: &str, at: f64) {
    if at == 0.0 || at.is_nan() || at > pr.now + 60.0 {
        return;
    }
    for w in pr.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()) {
        if w.panes.iter().any(|p| p.id == pane) && at > *w.activity {
            w.activity = at.floor().into();
        }
    }
}

/// The pane's last hook event, if it came from the agent running there now.
fn live_hook(pr: &ProbeResult, pane: &str) -> Option<HookPaneState> {
    let hook = pr.hook_panes.get(pane)?;
    let proc = pr.agent_procs.get(pane)?;
    (hook.ts >= proc.started_at - 5.0 && hook.from_agent(proc.kind)).then(|| hook.clone())
}

/// What runs in the background after a screen read. The footer that lists it is hidden while a
/// picker or a dialog is up, so what it last said stands until the footer shows again; a process
/// other than the one seen before starts clean. `screen`: None when the footer wasn't visible,
/// Some(None) when it listed nothing.
pub fn background_after(screen: Option<Option<&str>>, was: Option<(u32, Option<&str>)>, pid: u32) -> Option<String> {
    match screen {
        Some(s) => s.map(str::to_owned),
        None => was.filter(|(p, _)| *p == pid).and_then(|(_, b)| b.map(str::to_owned)),
    }
}

fn shown_state(s: Shown) -> AgentState {
    match s {
        Shown::Waiting => AgentState::Waiting,
        Shown::Working => AgentState::Working,
        Shown::Idle => AgentState::Idle,
    }
}

/// Copies what the screen says (state, mode, prompt summary) onto a pane's agent info. For Claude,
/// an answered prompt or a turn its hooks say is running keeps the state at working.
/// `base` is the mode known without the screen (see off_screen_mode).
fn apply_screen(p: &mut PaneInfo, mut sc: ScreenState, host: &Host, lines: &[String], hook: Option<&HookPaneState>, was: Option<&AgentPaneInfo>, base: Option<String>) {
    let host_id = host.id();
    let Some(info) = p.agent.as_mut() else { return };
    let same_proc = was.is_some_and(|w| w.pid == info.pid);
    // a prompt covers the footer that names the mode, so the last one seen stands until it's back
    let kept = if sc.prompt.is_some() && same_proc { was.and_then(|w| w.mode.clone()) } else { None };
    if is_driving(&host_id, &p.id) {
        sc.prompt = None;
    }
    // a question Codex asked mid-turn waits for you like a prompt does, even while it keeps working
    let shown = if sc.prompt.is_some() || sc.questions.is_some_and(|q| q != 0) {
        Shown::Waiting
    } else if sc.working {
        Shown::Working
    } else {
        Shown::Idle
    };
    // the agents whose hooks say when a turn starts and ends
    info.state = if matches!(info.kind, AgentKind::Claude | AgentKind::Copilot | AgentKind::Gemini | AgentKind::Antigravity) {
        shown_state(TURNS.with(|t| t.borrow_mut().state(&turn_key(&host_id, &p.id), shown, &lines.join("\n"), hook, now_ms())))
    } else {
        shown_state(shown)
    };
    if info.kind == AgentKind::Opencode {
        apply_open_code_screen(info, &sc, &host_id, host, was);
    } else {
        info.mode = sc.mode.clone().or(kept).or(base);
        // a session without live numbers still shows its model and effort on screen
        // agy's and Codex's footers are redrawn the moment you switch, so they beat numbers written earlier
        let live = matches!(info.kind, AgentKind::Antigravity | AgentKind::Codex);
        if let Some(m) = sc.model.clone().filter(|m| !m.is_empty()) {
            if live || info.status.as_ref().and_then(|s| s.model.as_deref()).is_none_or(str::is_empty) {
                info.status.get_or_insert_with(|| crate::agents::status::empty_status(0.0)).model = Some(m);
            }
        }
        if let Some(e) = sc.effort.clone().filter(|e| !e.is_empty()) {
            if live || info.status.as_ref().and_then(|s| s.effort.as_deref()).is_none_or(str::is_empty) {
                info.status.get_or_insert_with(|| crate::agents::status::empty_status(0.0)).effort = Some(e);
            }
        }
    }
    info.activity = sc.activity.clone();
    info.background = background_after(sc.background.as_ref().map(|b| b.as_deref()), was.map(|w| (w.pid, w.background.as_deref())), info.pid);
    // like the footer, the switch on the box can't be read while a dialog covers it
    if info.kind == AgentKind::Claude {
        info.ultracode = sc.ultracode.or(if same_proc { was.and_then(|w| w.ultracode) } else { None });
    }
    info.options = None;
    info.waiting_for = None;
    let Some(prompt) = &sc.prompt else {
        if let Some(q) = sc.questions.filter(|q| *q != 0) {
            info.waiting_for = Some(if q == 1 { "A question is waiting for your answer".into() } else { format!("{q} questions are waiting for your answer") });
        }
        return;
    };
    info.options = quick_options(prompt);
    info.waiting_for = Some(prompt_summary(prompt));
}

/// The answers the overview offers beside its one line (single-choice prompts only, up to four):
/// none when that line doesn't hold the whole question (`prompt_whole`).
pub fn quick_options(prompt: &ScreenPrompt) -> Option<Vec<QuickOption>> {
    if prompt.multi || prompt.tabs.as_ref().is_some_and(|t| !t.is_empty()) || prompt.input.is_some() || !prompt_whole(prompt) {
        return None;
    }
    Some(prompt.options.iter().filter(|o| Some(o.n) != prompt.free_text_option).take(4).map(|o| QuickOption { n: o.n as u32, label: o.label.clone() }).collect())
}

/// A host's settings as the browser sees them: the pairing token's hash stays on the hub.
pub fn public_config(c: &HostConfig) -> HostConfig {
    HostConfig { token_hash: None, ..c.clone() }
}

/// A pairing token works for a day if no machine connects with it; once one has, until it's removed.
pub const PAIR_TOKEN_MS: u64 = 24 * 3_600_000;

pub fn token_expired(c: &HostConfig, now: u64) -> bool {
    c.kind == HostKind::Node && c.paired_at.is_none_or(|p| p == 0) && c.token_issued_at.is_some_and(|t| t != 0 && now as f64 - t as f64 > PAIR_TOKEN_MS as f64)
}

/// OpenCode's footer (agent · model · variant) is the truth whenever it shows, in both modes: it
/// is what OpenCode itself runs the next message with. A menu, a permission or a question hides
/// it; the last values then stand for the same process, as Claude's status does above. No variant
/// on the footer means the model's default. The sidebar and status row give context and cost.
pub fn apply_open_code_screen(info: &mut AgentPaneInfo, sc: &ScreenState, host_id: &str, host: &Host, was: Option<&AgentPaneInfo>) {
    let same = was.filter(|w| w.kind == AgentKind::Opencode && w.pid == info.pid);
    let mut st = info.status.clone().or_else(|| same.and_then(|s| s.status.clone())).unwrap_or_else(|| crate::agents::status::empty_status(0.0));
    if let Some(model) = sc.model.as_deref().filter(|m| !m.is_empty()) {
        let models = peek_open_code_models(host_id).map(|c| c.models);
        // plain OpenCode has no API to ask: the CLI's list names the footer's model and its window
        if models.is_none() && !via_open_code_api(info.transcript_path.as_deref()) {
            let _ = opencode_cli_models(host);
        }
        let known = models.as_deref().and_then(|m| model_from_footer(m, model)).cloned();
        st.model = Some(match &known {
            Some(k) => k.label.split(" · ").skip(1).collect::<Vec<_>>().join(" · "),
            None => without_provider(model),
        });
        st.model_id = known.as_ref().map(|k| k.id.clone());
        st.effort = sc.effort.clone();
        if let Some(c) = known.as_ref().and_then(|k| k.context).filter(|c| **c != 0.0 && !c.is_nan()) {
            st.context_size = Some(c);
        }
        info.mode = sc.mode.clone();
    } else {
        info.mode = info.mode.clone().or_else(|| same.and_then(|s| s.mode.clone()));
    }
    if sc.context_pct.is_some() {
        st.context_pct = sc.context_pct;
    }
    if sc.context_used.is_some() {
        st.context_used = sc.context_used;
    }
    if sc.cost_usd.is_some() {
        st.cost_usd = sc.cost_usd;
    }
    st.updated_at = now_ms();
    info.status = Some(st);
}

/// A session with its own server says itself whether it's busy or waiting for a permission; the
/// footer still decides the model, agent and variant (see apply_open_code_screen), its server's
/// catalogue only puts the exact id and the window size to the footer's name. Without the server,
/// the pane stays as the terminal showed it.
async fn apply_open_code_live(host: &Host, mut pane: PaneInfo, sc: Option<&ScreenState>) -> PaneInfo {
    let Some(info) = pane.agent.as_ref() else { return pane };
    if info.kind != AgentKind::Opencode || !via_open_code_api(info.transcript_path.as_deref()) {
        return pane;
    }
    let uri = info.transcript_path.clone().unwrap_or_default();
    let cwd = Some(pane.cwd.clone()).filter(|c| !c.is_empty());
    let Ok(live) = opencode_live(host, &uri, cwd.as_deref()).await else { return pane };
    {
        let info = pane.agent.as_mut().expect("the agent");
        info.state = if live.permission.is_some() {
            AgentState::Waiting
        } else if live.working {
            AgentState::Working
        } else {
            AgentState::Idle
        };
        info.activity = if live.retrying {
            Some("Retrying".into())
        } else if live.working {
            Some("Working".into())
        } else {
            None
        };
        // its subagents' work counts as background, like a shell the footer lists
        info.background = live.background.clone();
        if let Some(perm) = &live.permission {
            // only the permission's kind ("bash"), not what it would allow: no quick answers, as
            // for any question the one line doesn't hold (prompt_whole)
            info.waiting_for = Some(Some(perm).at("permission").filter(|v| !v.is_null()).map_or_else(|| "Permission requested".to_owned(), json::value_string));
            info.options = None;
        }
    }
    let needs_id = pane.agent.as_ref().and_then(|a| a.status.as_ref()).is_some_and(|s| s.model_id.is_none());
    if let (Some(model), true) = (sc.and_then(|s| s.model.as_deref()).filter(|m| !m.is_empty()), needs_id) {
        let Ok(catalog) = opencode_catalog_cached(host, &uri, cwd.as_deref()).await else { return pane };
        if let Some(known) = model_from_footer(&catalog.models, model) {
            if let Some(st) = pane.agent.as_mut().and_then(|a| a.status.as_mut()) {
                st.model = Some(known.label.split(" · ").skip(1).collect::<Vec<_>>().join(" · "));
                st.model_id = Some(known.id.clone());
                if let Some(c) = known.context.filter(|c| **c != 0.0 && !c.is_nan()) {
                    st.context_size = Some(c);
                }
            }
        }
    }
    pane
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
