//! How sessions are organised (from 0.24), kept by the hub so every device shows the same: pins in
//! an order, parked and snoozed sessions, tags, notes, titles and sections on sessions, and
//! projects (with groups and spaces) that sessions fall into by their folder.
//!
//! A session is a pane, and panes don't keep their ids for ever: tmux numbers them again from %0
//! after its server restarts. So a mark keeps anchors and is matched to a live pane on every
//! overview ([`reconcile`]): first by the agent's conversation id, which follows a conversation
//! resumed in another pane, then by the pane id together with the tmux session it was in, so a
//! reused id in another session isn't taken for the old pane, and last by its place (the tmux
//! session's name, the window and pane numbers and the folder), which finds a session that
//! tmux-resurrect or a script put back after tmux restarted. A mark that matches nothing stays,
//! not `live`, for when its pane comes back (a machine that was off), and goes after 30 days.
//!
//! Kept in the `organize` record with a version (see [`ORGANIZE_VERSION`]); a record written by a
//! later hub is read but never written over. What the hub does on its own with it (messages sent
//! later, agents put to sleep, sessions closed when done, saved prompts) is in [`auto`].

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use inyon_protocol::{
    js_len, normalize_tag, squash_spaces, HostState, HostStatus, Organize, PaneInfo, Project, ProjectColor, ProjectGroup, ProjectRoot, SessionInfo, SessionMark, Space, WindowInfo,
    ICON_LEN, MAX_GROUPS, MAX_MARKS, MAX_PROJECTS, MAX_ROOTS, MAX_SPACES, MAX_TAGS, NAME_LEN, NOTE_LEN, ORGANIZE_VERSION, PATH_LEN, SECTION_LEN, TAG_LEN, TITLE_LEN,
};
use serde_json::{Map, Value};

use crate::clock::now_ms;
use crate::hosts::http_error::{http, HubError, HubResult};
use crate::hosts::registry::registry;
use crate::platform::store::kv;

pub const KEY: &str = "organize";
/// Where a record this hub couldn't read is put aside, rather than written over.
pub const UNREADABLE_KEY: &str = "organize.unreadable";
/// A mark not seen for this long goes.
pub const FORGET_MS: u64 = 30 * 24 * 3_600_000;
/// `seen` is written down at most this often while nothing else about a mark changes.
const SEEN_EVERY_MS: u64 = 3_600_000;
/// A snooze can last this long at most.
pub const MAX_SNOOZE_MS: u64 = 366 * 24 * 3_600_000;
/// How often the hub looks for snoozes that are over, messages due and idle agents.
pub const TICK_MS: u64 = 5_000;

/// The record couldn't be read, and couldn't be copied aside either, so it isn't written over.
pub const UNREADABLE: &str = "Inyon couldn't read how your sessions are organised, nor keep a copy of it, so it won't save over it";
pub const NEWER: &str = "A newer Inyon hub saved how your sessions are organised. Update this hub to change it.";

/// What the apps hear of.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// something they show changed: send it on
    Changed,
    /// a snooze that asked for one is over: a notice for the session
    Notice { host: String, pane: String, title: String, body: String },
}

struct State {
    org: Organize,
    /// written by a later hub: read only
    newer: bool,
    /// a record this hub couldn't read and couldn't yet put aside: nothing is written over it
    /// until it has been
    aside: Option<Value>,
}

type Listener = Rc<dyn Fn(&Event)>;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

fn load() -> State {
    let Some(raw) = kv::get_value(KEY) else { return State { org: Organize { v: ORGANIZE_VERSION, ..Organize::default() }, newer: false, aside: None } };
    match serde_json::from_value::<Organize>(raw.clone()) {
        Ok(mut org) => {
            let newer = org.v > ORGANIZE_VERSION;
            if !newer {
                org = migrate(org);
            }
            // nothing is live until the first overview says so
            for m in &mut org.sessions {
                m.live = false;
            }
            State { org, newer, aside: None }
        }
        Err(e) => {
            crate::platform::console::error(&format!("the organize record couldn't be read ({e}); it goes aside as {UNREADABLE_KEY}"));
            let mut st = State { org: Organize { v: ORGANIZE_VERSION, ..Organize::default() }, newer: false, aside: Some(raw) };
            if let Err(e) = put_aside(&mut st) {
                crate::platform::console::error(&e.message);
            }
            st
        }
    }
}

/// Brings a record from an earlier version up to this one. Version 0 is a record from before
/// versions were written, which never shipped; it reads as version 1. Version 2 added messages
/// sent later, sleep rules, saved prompts and closing when done, all of them new fields, so a
/// version 1 record reads as it is.
pub fn migrate(mut org: Organize) -> Organize {
    if org.v < 2 {
        org.v = 2;
    }
    org
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(s.borrow_mut().get_or_insert_with(load)))
}

/// Puts an unreadable record aside, if there's one still waiting to be.
fn put_aside(st: &mut State) -> HubResult<()> {
    let Some(raw) = &st.aside else { return Ok(()) };
    kv::set_value(UNREADABLE_KEY, raw).map_err(|e| HubError::plain(format!("{UNREADABLE}: {e}")))?;
    st.aside = None;
    Ok(())
}

fn save(st: &mut State) -> HubResult<()> {
    if st.newer {
        return Err(http(409, NEWER));
    }
    put_aside(st)?;
    let v = serde_json::to_value(&st.org).map_err(|e| HubError::plain(e.to_string()))?;
    kv::set_value(KEY, &v).map_err(|e| HubError::plain(format!("couldn't keep how the sessions are organised: {e}")))
}

fn emit(e: Event) {
    let all: Vec<Listener> = LISTENERS.with(|l| l.borrow().clone());
    for f in all {
        f(&e);
    }
}

/// Hears every change and notice (the apps' sockets).
pub fn on_event(f: impl Fn(&Event) + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Rc::new(f)));
}

/// Changes the record, writes it and tells the apps; nothing changes when it can't be written.
/// Like [`change`], for what mustn't be lost: when it can't be written it stays in memory, to be
/// written with the next save, and the error says so.
fn change_or_keep(f: impl FnOnce(&mut Organize)) -> HubResult<()> {
    let out = with(|st| {
        if st.newer {
            return Err(http(409, NEWER));
        }
        f(&mut st.org);
        save(st)
    });
    emit(Event::Changed);
    out
}

fn change<T>(f: impl FnOnce(&mut Organize) -> HubResult<T>) -> HubResult<T> {
    let out = with(|st| {
        if st.newer {
            return Err(http(409, NEWER));
        }
        let before = st.org.clone();
        let out = match f(&mut st.org) {
            Ok(out) => out,
            Err(e) => {
                // a change refused half way leaves nothing behind
                st.org = before;
                return Err(e);
            }
        };
        if let Err(e) = save(st) {
            st.org = before;
            return Err(e);
        }
        Ok(out)
    })?;
    emit(Event::Changed);
    Ok(out)
}

/// Everything, as GET /api/organize and the `/ws2` stream have it.
pub fn get() -> Organize {
    with(|st| st.org.clone())
}

/// What a v1 overview carries (see [`auto::for_overview`]).
pub fn for_overview() -> Organize {
    auto::for_overview(get())
}

/// The same, matched to the machines as they are now.
pub fn current() -> Organize {
    reconcile(&registry().overview());
    get()
}

/// Forgets what was read, so the next call reads the record again (tests, and a hub that restarts in place).
pub fn reset() {
    STATE.with(|s| *s.borrow_mut() = None);
}

// ---------- matching marks to panes ----------

/// A pane on a machine, with the tmux session it's in.
struct Live<'a> {
    pane: &'a PaneInfo,
    window: &'a WindowInfo,
    session: &'a SessionInfo,
}

impl Live<'_> {
    /// "window.pane", tmux's numbers
    fn place(&self) -> String {
        format!("{}.{}", fmt_index(*self.window.index), fmt_index(*self.pane.index))
    }
}

fn fmt_index(n: f64) -> String {
    if n.is_finite() {
        format!("{}", n as i64)
    } else {
        "?".into()
    }
}

fn conv_of(p: &PaneInfo) -> Option<&str> {
    p.agent.as_ref().and_then(|a| a.session_id.as_deref()).filter(|s| !s.is_empty())
}

/// Matches every mark to a pane on its machine and notes where each was found. Returns whether
/// anything the apps show changed; writes the record when an anchor moved. Marks on a machine
/// that isn't online stay as they were until it is: its panes are the last it reported.
pub fn reconcile(hosts: &[HostState]) -> bool {
    let now = now_ms();
    with(|st| {
        let (mut shown, mut keep) = match_marks(&mut st.org.sessions, hosts, now);
        // marks of panes long gone
        let n = st.org.sessions.len();
        st.org.sessions.retain(|m| m.live || now.saturating_sub(m.seen) < FORGET_MS);
        if st.org.sessions.len() != n {
            renumber_pins(&mut st.org);
            (shown, keep) = (true, true);
        }
        if keep && !st.newer {
            if let Err(e) = save(st) {
                crate::platform::console::error(&e.message);
            }
        }
        shown
    })
}

/// The matching itself: (an app sees a change, something worth writing down changed).
fn match_marks(marks: &mut [SessionMark], hosts: &[HostState], now: u64) -> (bool, bool) {
    let mut shown = false;
    let mut keep = false;
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    let mut found: Vec<Option<(usize, usize)>> = vec![None; marks.len()];
    let lives: Vec<Vec<Live>> = hosts
        .iter()
        .map(|h| h.sessions.iter().flat_map(|s| s.windows.iter().flat_map(move |w| w.panes.iter().map(move |p| Live { pane: p, window: w, session: s }))).collect())
        .collect();
    let host_at = |id: &str| hosts.iter().position(|h| h.config.id == id);
    let away = |m: &SessionMark| host_at(&m.host).is_some_and(|h| hosts[h].status != HostStatus::Online);
    // first by conversation: it follows the conversation wherever it was resumed
    for (i, m) in marks.iter().enumerate() {
        let (Some(conv), Some(h)) = (m.conv.as_deref(), host_at(&m.host)) else { continue };
        if away(m) {
            continue;
        }
        if let Some(j) = lives[h].iter().position(|l| conv_of(l.pane) == Some(conv) && !claimed.contains(&(m.host.clone(), l.pane.id.clone()))) {
            claimed.insert((m.host.clone(), lives[h][j].pane.id.clone()));
            found[i] = Some((h, j));
        }
    }
    // then by the pane id, in the tmux session it was in: the same session id (so a renamed one
    // still counts), or the same name in the same folder. A name alone isn't enough: after tmux
    // restarts, a new session of the same name can get an old pane id.
    for (i, m) in marks.iter().enumerate() {
        if found[i].is_some() || away(m) {
            continue;
        }
        let Some(h) = host_at(&m.host) else { continue };
        let same_place = |l: &Live| {
            (m.tmux_id.is_none() && m.tmux_name.is_none())
                || m.tmux_id.as_deref() == Some(l.session.id.as_str())
                || (m.tmux_name.as_deref() == Some(l.session.name.as_str()) && m.cwd.as_deref() == Some(l.pane.cwd.as_str()))
        };
        if let Some(j) = lives[h].iter().position(|l| l.pane.id == m.pane && same_place(l) && !claimed.contains(&(m.host.clone(), l.pane.id.clone()))) {
            claimed.insert((m.host.clone(), lives[h][j].pane.id.clone()));
            found[i] = Some((h, j));
        }
    }
    // last by place: put back after tmux restarted, under new ids
    for (i, m) in marks.iter().enumerate() {
        let (None, Some(h), Some(name), Some(place)) = (found[i], host_at(&m.host), m.tmux_name.as_deref(), m.tmux_place.as_deref()) else { continue };
        if away(m) {
            continue;
        }
        if let Some(j) = lives[h]
            .iter()
            .position(|l| l.session.name == name && l.place() == place && m.cwd.as_deref() == Some(l.pane.cwd.as_str()) && !claimed.contains(&(m.host.clone(), l.pane.id.clone())))
        {
            claimed.insert((m.host.clone(), lives[h][j].pane.id.clone()));
            found[i] = Some((h, j));
        }
    }
    for (i, m) in marks.iter_mut().enumerate() {
        if away(m) {
            continue;
        }
        let Some((h, j)) = found[i] else {
            if m.live {
                m.live = false;
                shown = true;
            }
            continue;
        };
        let l = &lives[h][j];
        let conv = conv_of(l.pane).map(str::to_owned).or_else(|| m.conv.clone());
        let place = l.place();
        let anchors = (l.pane.id.as_str(), conv.as_deref(), Some(l.session.id.as_str()), Some(l.session.name.as_str()), Some(place.as_str()), Some(l.pane.cwd.as_str()));
        let mut write = now.saturating_sub(m.seen) >= SEEN_EVERY_MS;
        if anchors != (m.pane.as_str(), m.conv.as_deref(), m.tmux_id.as_deref(), m.tmux_name.as_deref(), m.tmux_place.as_deref(), m.cwd.as_deref()) {
            // the apps join by pane id, so a new one is news to them
            shown |= m.pane != l.pane.id;
            m.pane = l.pane.id.clone();
            m.conv = conv;
            m.tmux_id = Some(l.session.id.clone());
            m.tmux_name = Some(l.session.name.clone());
            m.tmux_place = Some(place);
            m.cwd = Some(l.pane.cwd.clone());
            write = true;
        }
        if !m.live {
            m.live = true;
            shown = true;
        }
        if write {
            m.seen = now;
            keep = true;
        }
    }
    (shown, keep)
}

// ---------- reading what the app sent ----------

/// A field as given: left out, `null`, or a value.
enum Given<'a> {
    Absent,
    Null,
    Some(&'a Value),
}

fn given<'a>(b: &'a Map<String, Value>, k: &str) -> Given<'a> {
    match b.get(k) {
        None => Given::Absent,
        Some(Value::Null) => Given::Null,
        Some(v) => Given::Some(v),
    }
}

fn bool_field(b: &Map<String, Value>, k: &str) -> HubResult<Option<bool>> {
    match given(b, k) {
        Given::Absent => Ok(None),
        Given::Null => Ok(Some(false)),
        Given::Some(Value::Bool(v)) => Ok(Some(*v)),
        Given::Some(_) => Err(http(400, format!("{k} must be true or false"))),
    }
}

/// A short line of text: spaces squashed; empty or null clears it. Too long is refused.
fn line_field(b: &Map<String, Value>, k: &str, max: usize, what: &str) -> HubResult<Option<Option<String>>> {
    match given(b, k) {
        Given::Absent => Ok(None),
        Given::Null => Ok(Some(None)),
        Given::Some(Value::String(s)) => {
            let s = squash_spaces(s);
            if js_len(&s) > max {
                return Err(http(400, format!("{what} can be at most {max} characters")));
            }
            Ok(Some(Some(s).filter(|s| !s.is_empty())))
        }
        Given::Some(_) => Err(http(400, format!("{k} must be text"))),
    }
}

fn tags_field(b: &Map<String, Value>) -> HubResult<Option<Vec<String>>> {
    let list = match given(b, "tags") {
        Given::Absent => return Ok(None),
        Given::Null => return Ok(Some(vec![])),
        Given::Some(Value::Array(l)) => l,
        Given::Some(_) => return Err(http(400, "tags must be a list of words")),
    };
    let mut out: Vec<String> = Vec::new();
    for t in list {
        let Some(raw) = t.as_str() else { return Err(http(400, "tags must be a list of words")) };
        if raw.trim().trim_start_matches('#').trim().is_empty() {
            continue;
        }
        let Some(tag) = normalize_tag(raw) else { return Err(http(400, format!("A tag can be at most {TAG_LEN} characters, on one line"))) };
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(http(400, format!("A session can have at most {MAX_TAGS} tags")));
    }
    Ok(Some(out))
}

/// A change to a session's mark, as `PATCH …/organize` sends it. A field left out stays as it is;
/// `null` clears it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionPatch {
    pub pinned: Option<bool>,
    pub parked: Option<bool>,
    pub snoozed_until: Option<Option<u64>>,
    pub snooze_notice: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub note: Option<Option<String>>,
    pub title: Option<Option<String>>,
    pub section: Option<Option<String>>,
    pub project: Option<Option<String>>,
    pub close_when_done: Option<bool>,
}

impl SessionPatch {
    /// Reads and checks a request body; `now` is for the snooze.
    pub fn read(b: &Value, now: u64) -> HubResult<SessionPatch> {
        let Some(b) = b.as_object() else { return Err(http(400, "Expected a JSON object")) };
        let snoozed_until = match given(b, "snoozedUntil") {
            Given::Absent => None,
            Given::Null => Some(None),
            Given::Some(v) => {
                let at = v.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| http(400, "snoozedUntil must be a time (unix ms)"))? as u64;
                if at <= now {
                    return Err(http(400, "Pick a time in the future to snooze until"));
                }
                if at - now > MAX_SNOOZE_MS {
                    return Err(http(400, "A snooze can last a year at most"));
                }
                Some(Some(at))
            }
        };
        let note = match given(b, "note") {
            Given::Absent => None,
            Given::Null => Some(None),
            Given::Some(Value::String(s)) => {
                let s = s.trim();
                if js_len(s) > NOTE_LEN {
                    return Err(http(400, format!("A note can be at most {NOTE_LEN} characters")));
                }
                Some(Some(s.to_owned()).filter(|s| !s.is_empty()))
            }
            Given::Some(_) => return Err(http(400, "note must be text")),
        };
        let project = match given(b, "project") {
            Given::Absent => None,
            Given::Null => Some(None),
            Given::Some(Value::String(s)) => Some(Some(s.clone()).filter(|s| !s.is_empty())),
            Given::Some(_) => return Err(http(400, "project must be a project's id")),
        };
        Ok(SessionPatch {
            pinned: bool_field(b, "pinned")?,
            parked: bool_field(b, "parked")?,
            snoozed_until,
            snooze_notice: bool_field(b, "snoozeNotice")?,
            tags: tags_field(b)?,
            note,
            title: line_field(b, "title", TITLE_LEN, "A title")?,
            section: line_field(b, "section", SECTION_LEN, "A section's name")?,
            project,
            close_when_done: bool_field(b, "closeWhenDone")?,
        })
    }
}

/// Pinned marks get places 0, 1, 2… in the order they have now.
fn renumber_pins(org: &mut Organize) {
    let mut pinned: Vec<(u32, usize)> = org.sessions.iter().enumerate().filter_map(|(i, m)| m.pinned.map(|p| (p, i))).collect();
    pinned.sort();
    for (n, (_, i)) in pinned.into_iter().enumerate() {
        org.sessions[i].pinned = Some(n as u32);
    }
}

fn apply(org: &mut Organize, i: usize, p: &SessionPatch) -> HubResult<()> {
    if let Some(Some(pid)) = &p.project {
        if !org.projects.iter().any(|x| &x.id == pid) {
            return Err(http(404, "There's no such project"));
        }
    }
    let next_pin = org.sessions.iter().filter_map(|m| m.pinned).max().map_or(0, |n| n + 1);
    let m = &mut org.sessions[i];
    match p.pinned {
        Some(true) if m.pinned.is_none() => m.pinned = Some(next_pin),
        Some(false) => m.pinned = None,
        _ => {}
    }
    if let Some(v) = p.parked {
        m.parked = v.then_some(true);
    }
    if let Some(v) = p.snoozed_until {
        m.snoozed_until = v;
        if v.is_none() {
            m.snooze_notice = None;
        }
    }
    if let Some(v) = p.snooze_notice {
        m.snooze_notice = (v && m.snoozed_until.is_some()).then_some(true);
    }
    if let Some(t) = &p.tags {
        m.tags = (!t.is_empty()).then(|| t.clone());
    }
    if let Some(v) = p.close_when_done {
        // the agent's next piece of work counts from now
        m.close_when_done = v.then_some(true);
        m.close_saw_work = None;
    }
    for (to, from) in [(&mut m.note, &p.note), (&mut m.title, &p.title), (&mut m.section, &p.section), (&mut m.project, &p.project)] {
        if let Some(v) = from {
            *to = v.clone();
        }
    }
    renumber_pins(org);
    Ok(())
}

fn new_id(prefix: &str) -> String {
    let hex: String = crate::ids::random_uuid().chars().filter(char::is_ascii_hexdigit).take(12).collect();
    format!("{prefix}-{hex}")
}

/// Keeps the list within its limit once a new mark is in: the one seen longest ago that isn't
/// live goes. The new mark is live, so it's never the one.
fn make_room(org: &mut Organize) -> HubResult<()> {
    if org.sessions.len() <= MAX_MARKS {
        return Ok(());
    }
    let oldest = org.sessions.iter().enumerate().filter(|(_, m)| !m.live).min_by_key(|(_, m)| m.seen).map(|(i, _)| i);
    match oldest {
        Some(i) => {
            org.sessions.remove(i);
            renumber_pins(org);
            Ok(())
        }
        None => Err(http(409, format!("Inyon keeps what you set on {MAX_MARKS} sessions at most"))),
    }
}

/// The result of a change to one session: its mark, or None when nothing is left on it.
fn finish(org: &mut Organize, i: usize) -> Option<SessionMark> {
    if org.sessions[i].is_blank() {
        org.sessions.remove(i);
        renumber_pins(org);
        return None;
    }
    Some(org.sessions[i].clone())
}

/// Changes what's kept about a pane's session, making its mark if it has none.
pub fn mark_pane(host_id: &str, pane_id: &str, patch: &SessionPatch) -> HubResult<Option<SessionMark>> {
    reconcile(&registry().overview());
    let found = registry().find_pane(host_id, pane_id);
    change(|org| {
        let i = match org.sessions.iter().position(|m| m.live && m.host == host_id && m.pane == pane_id) {
            Some(i) => i,
            None => {
                let f = found.as_ref().ok_or_else(|| http(404, "Pane not found"))?;
                org.sessions.push(SessionMark {
                    id: new_id("s"),
                    host: host_id.to_owned(),
                    pane: pane_id.to_owned(),
                    live: true,
                    conv: conv_of(&f.pane).map(str::to_owned),
                    tmux_id: Some(f.session.id.clone()),
                    tmux_name: Some(f.session.name.clone()),
                    tmux_place: Some(Live { pane: &f.pane, window: &f.window, session: &f.session }.place()),
                    cwd: Some(f.pane.cwd.clone()),
                    seen: now_ms(),
                    pinned: None,
                    parked: None,
                    snoozed_until: None,
                    snooze_notice: None,
                    tags: None,
                    note: None,
                    title: None,
                    section: None,
                    project: None,
                    close_when_done: None,
                    close_saw_work: None,
                });
                org.sessions.len() - 1
            }
        };
        apply(org, i, patch)?;
        let Some(mark) = finish(org, i) else { return Ok(None) };
        // room is made only for a mark that keeps something, so a request that keeps nothing
        // can't cost another session its mark
        make_room(org)?;
        Ok(Some(mark))
    })
}

/// Changes a mark by its id, whether or not its pane is there now.
pub fn mark_by_id(id: &str, patch: &SessionPatch) -> HubResult<Option<SessionMark>> {
    change(|org| {
        let i = org.sessions.iter().position(|m| m.id == id).ok_or_else(|| http(404, "That session isn't kept any more"))?;
        apply(org, i, patch)?;
        Ok(finish(org, i))
    })
}

/// Forgets everything kept about a session.
pub fn forget(id: &str) -> HubResult<()> {
    change(|org| {
        let i = org.sessions.iter().position(|m| m.id == id).ok_or_else(|| http(404, "That session isn't kept any more"))?;
        org.sessions.remove(i);
        renumber_pins(org);
        Ok(())
    })
}

// ---------- projects, groups, spaces ----------

fn name_field(b: &Map<String, Value>, required: bool, what: &str) -> HubResult<Option<String>> {
    match line_field(b, "name", NAME_LEN, "A name")? {
        Some(Some(n)) => Ok(Some(n)),
        Some(None) => Err(http(400, format!("Give the {what} a name"))),
        None if required => Err(http(400, format!("Give the {what} a name"))),
        None => Ok(None),
    }
}

fn color_field(b: &Map<String, Value>) -> HubResult<Option<ProjectColor>> {
    match given(b, "color") {
        Given::Absent => Ok(None),
        Given::Null => Ok(Some(ProjectColor::default())),
        Given::Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|_| http(400, "Unknown colour")),
    }
}

fn icon_field(b: &Map<String, Value>) -> HubResult<Option<Option<String>>> {
    let icon = line_field(b, "icon", ICON_LEN, "An icon")?;
    if let Some(Some(i)) = &icon {
        if i.contains(' ') {
            return Err(http(400, "An icon is one emoji or an icon's name"));
        }
    }
    Ok(icon)
}

fn absolute(p: &str) -> bool {
    p.starts_with('/') || p.as_bytes().get(1) == Some(&b':') && matches!(p.as_bytes().get(2), Some(b'\\' | b'/'))
}

fn roots_field(b: &Map<String, Value>) -> HubResult<Option<Vec<ProjectRoot>>> {
    let list = match given(b, "roots") {
        Given::Absent => return Ok(None),
        Given::Null => return Ok(Some(vec![])),
        Given::Some(Value::Array(l)) => l,
        Given::Some(_) => return Err(http(400, "roots must be a list of folders")),
    };
    if list.len() > MAX_ROOTS {
        return Err(http(400, format!("A project can have at most {MAX_ROOTS} folders")));
    }
    let mut out: Vec<ProjectRoot> = Vec::new();
    for r in list {
        let (Some(host), Some(path)) = (r.get("host").and_then(Value::as_str), r.get("path").and_then(Value::as_str)) else {
            return Err(http(400, "Each folder needs a machine and a path"));
        };
        let path = path.trim();
        if host.is_empty() || !absolute(path) || js_len(path) > PATH_LEN || path.chars().any(char::is_control) {
            return Err(http(400, "A project's folder is a full path on one machine"));
        }
        let root = ProjectRoot { host: host.to_owned(), path: path.to_owned() };
        if !out.contains(&root) {
            out.push(root);
        }
    }
    Ok(Some(out))
}

fn ref_field(b: &Map<String, Value>, k: &str) -> HubResult<Option<Option<String>>> {
    match given(b, k) {
        Given::Absent => Ok(None),
        Given::Null => Ok(Some(None)),
        Given::Some(Value::String(s)) => Ok(Some(Some(s.clone()).filter(|s| !s.is_empty()))),
        Given::Some(_) => Err(http(400, format!("{k} must be an id"))),
    }
}

fn object(b: &Value) -> HubResult<&Map<String, Value>> {
    b.as_object().ok_or_else(|| http(400, "Expected a JSON object"))
}

fn set_project(org: &Organize, p: &mut Project, b: &Map<String, Value>, new: bool) -> HubResult<()> {
    if let Some(n) = name_field(b, new, "project")? {
        p.name = n;
    }
    if let Some(c) = color_field(b)? {
        p.color = c;
    }
    if let Some(i) = icon_field(b)? {
        p.icon = i;
    }
    if let Some(r) = roots_field(b)? {
        // a folder on a machine this hub doesn't have would never match a session; one kept from
        // before stays even when its machine has since gone
        if let Some(bad) = r.iter().find(|x| !p.roots.contains(x) && registry().get(&x.host).is_none()) {
            return Err(http(404, format!("Unknown machine: {}", bad.host)));
        }
        p.roots = r;
    }
    if let Some(g) = ref_field(b, "group")? {
        if g.as_ref().is_some_and(|g| !org.groups.iter().any(|x| &x.id == g)) {
            return Err(http(404, "There's no such group"));
        }
        p.group = g;
    }
    Ok(())
}

pub fn add_project(b: &Value) -> HubResult<Project> {
    let b = object(b)?;
    change(|org| {
        if org.projects.len() >= MAX_PROJECTS {
            return Err(http(409, format!("Inyon keeps {MAX_PROJECTS} projects at most")));
        }
        let mut p = Project { id: new_id("p"), name: String::new(), color: ProjectColor::default(), icon: None, roots: vec![], group: None };
        set_project(org, &mut p, b, true)?;
        org.projects.push(p.clone());
        Ok(p)
    })
}

pub fn update_project(id: &str, b: &Value) -> HubResult<Project> {
    let b = object(b)?;
    change(|org| {
        let i = org.projects.iter().position(|p| p.id == id).ok_or_else(|| http(404, "There's no such project"))?;
        let mut p = org.projects[i].clone();
        set_project(org, &mut p, b, false)?;
        org.projects[i] = p.clone();
        Ok(p)
    })
}

/// Removes a project; its sessions fall back to the project their folder is in, and spaces drop it.
pub fn remove_project(id: &str) -> HubResult<()> {
    change(|org| {
        let n = org.projects.len();
        org.projects.retain(|p| p.id != id);
        if org.projects.len() == n {
            return Err(http(404, "There's no such project"));
        }
        for m in &mut org.sessions {
            if m.project.as_deref() == Some(id) {
                m.project = None;
            }
        }
        for s in &mut org.spaces {
            s.projects.retain(|p| p != id);
        }
        auto::project_removed(org, id);
        org.sessions.retain(|m| !m.is_blank());
        renumber_pins(org);
        Ok(())
    })
}

pub fn add_group(b: &Value) -> HubResult<ProjectGroup> {
    let b = object(b)?;
    change(|org| {
        if org.groups.len() >= MAX_GROUPS {
            return Err(http(409, format!("Inyon keeps {MAX_GROUPS} groups at most")));
        }
        let name = name_field(b, true, "group")?.unwrap_or_default();
        let g = ProjectGroup { id: new_id("g"), name, color: color_field(b)?.unwrap_or_default() };
        org.groups.push(g.clone());
        Ok(g)
    })
}

pub fn update_group(id: &str, b: &Value) -> HubResult<ProjectGroup> {
    let b = object(b)?;
    change(|org| {
        let g = org.groups.iter_mut().find(|g| g.id == id).ok_or_else(|| http(404, "There's no such group"))?;
        if let Some(n) = name_field(b, false, "group")? {
            g.name = n;
        }
        if let Some(c) = color_field(b)? {
            g.color = c;
        }
        Ok(g.clone())
    })
}

/// Removes a group; its projects stay, outside any group.
pub fn remove_group(id: &str) -> HubResult<()> {
    change(|org| {
        let n = org.groups.len();
        org.groups.retain(|g| g.id != id);
        if org.groups.len() == n {
            return Err(http(404, "There's no such group"));
        }
        for p in &mut org.projects {
            if p.group.as_deref() == Some(id) {
                p.group = None;
            }
        }
        Ok(())
    })
}

fn space_projects(org: &Organize, b: &Map<String, Value>) -> HubResult<Option<Vec<String>>> {
    let list = match given(b, "projects") {
        Given::Absent => return Ok(None),
        Given::Null => return Ok(Some(vec![])),
        Given::Some(Value::Array(l)) => l,
        Given::Some(_) => return Err(http(400, "projects must be a list of project ids")),
    };
    let mut out: Vec<String> = Vec::new();
    for p in list {
        let id = p.as_str().ok_or_else(|| http(400, "projects must be a list of project ids"))?;
        if !org.projects.iter().any(|x| x.id == id) {
            return Err(http(404, "There's no such project"));
        }
        if !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    }
    Ok(Some(out))
}

pub fn add_space(b: &Value) -> HubResult<Space> {
    let b = object(b)?;
    change(|org| {
        if org.spaces.len() >= MAX_SPACES {
            return Err(http(409, format!("Inyon keeps {MAX_SPACES} spaces at most")));
        }
        let name = name_field(b, true, "space")?.unwrap_or_default();
        let s = Space { id: new_id("w"), name, icon: icon_field(b)?.flatten(), projects: space_projects(org, b)?.unwrap_or_default() };
        org.spaces.push(s.clone());
        Ok(s)
    })
}

pub fn update_space(id: &str, b: &Value) -> HubResult<Space> {
    let b = object(b)?;
    change(|org| {
        let projects = space_projects(org, b)?;
        let s = org.spaces.iter_mut().find(|s| s.id == id).ok_or_else(|| http(404, "There's no such space"))?;
        if let Some(n) = name_field(b, false, "space")? {
            s.name = n;
        }
        if let Some(i) = icon_field(b)? {
            s.icon = i;
        }
        if let Some(p) = projects {
            s.projects = p;
        }
        Ok(s.clone())
    })
}

pub fn remove_space(id: &str) -> HubResult<()> {
    change(|org| {
        let n = org.spaces.len();
        org.spaces.retain(|s| s.id != id);
        if org.spaces.len() == n {
            return Err(http(404, "There's no such space"));
        }
        Ok(())
    })
}

/// The ids listed in `b[k]`, if given.
fn id_list(b: &Map<String, Value>, k: &str) -> HubResult<Option<Vec<String>>> {
    match given(b, k) {
        Given::Absent | Given::Null => Ok(None),
        Given::Some(Value::Array(l)) => {
            l.iter().map(|v| v.as_str().map(str::to_owned).ok_or_else(|| http(400, format!("{k} must be a list of ids")))).collect::<HubResult<Vec<_>>>().map(Some)
        }
        Given::Some(_) => Err(http(400, format!("{k} must be a list of ids"))),
    }
}

/// Puts the listed ones first, in that order; the rest keep their order after them.
fn reorder<T>(list: &mut Vec<T>, ids: &[String], id: impl Fn(&T) -> &str) {
    let mut rest: Vec<T> = std::mem::take(list);
    for want in ids {
        if let Some(i) = rest.iter().position(|x| id(x) == want) {
            list.push(rest.remove(i));
        }
    }
    list.append(&mut rest);
}

/// `PUT /api/organize/order`: the order of pinned sessions (by mark id), projects, groups, spaces
/// and saved prompts.
pub fn set_order(b: &Value) -> HubResult<()> {
    let b = object(b)?;
    let (pins, projects, groups, spaces, prompts) = (id_list(b, "pins")?, id_list(b, "projects")?, id_list(b, "groups")?, id_list(b, "spaces")?, id_list(b, "prompts")?);
    change(|org| {
        if let Some(pins) = pins {
            let mut order: Vec<String> = pins.into_iter().filter(|id| org.sessions.iter().any(|m| &m.id == id && m.pinned.is_some())).collect();
            let mut rest: Vec<(u32, String)> =
                org.sessions.iter().filter(|m| m.pinned.is_some() && !order.contains(&m.id)).map(|m| (m.pinned.unwrap_or(0), m.id.clone())).collect();
            rest.sort();
            order.extend(rest.into_iter().map(|(_, id)| id));
            for m in &mut org.sessions {
                if let Some(n) = order.iter().position(|id| *id == m.id) {
                    m.pinned = Some(n as u32);
                }
            }
        }
        if let Some(ids) = projects {
            reorder(&mut org.projects, &ids, |p| &p.id);
        }
        if let Some(ids) = groups {
            reorder(&mut org.groups, &ids, |g| &g.id);
        }
        if let Some(ids) = spaces {
            reorder(&mut org.spaces, &ids, |s| &s.id);
        }
        if let (Some(ids), Some(list)) = (prompts, org.prompts.as_mut()) {
            reorder(list, &ids, |q| &q.id);
        }
        Ok(())
    })
}

// ---------- the clock ----------

/// What a session is called in a notice: its title in Inyon, else its pane's.
fn name_for(m: &SessionMark, hosts: &[HostState]) -> String {
    if let Some(t) = &m.title {
        return t.clone();
    }
    let pane = hosts.iter().filter(|h| h.config.id == m.host).flat_map(|h| h.sessions.iter().flat_map(|s| s.windows.iter().flat_map(|w| w.panes.iter()))).find(|p| p.id == m.pane);
    pane.map(|p| p.label.clone().unwrap_or_else(|| crate::hosts::registry::pane_title(p))).filter(|t| !t.is_empty()).unwrap_or_else(|| "A session".to_owned())
}

/// Ends the snoozes that are over, raising the notices they asked for. Whether any ended.
pub fn wake_due(now: u64) -> bool {
    let hosts = registry().overview();
    // a session on a machine that isn't online wakes when the machine is back, so its notice
    // is about a session you can open
    let away = |m: &SessionMark| hosts.iter().any(|h| h.config.id == m.host && h.status != HostStatus::Online);
    let ready = |m: &SessionMark| m.snoozed_until.is_some_and(|t| t <= now) && !away(m);
    let due = with(|st| st.org.sessions.iter().any(ready) && !st.newer);
    if !due {
        return false;
    }
    let woke = change(|org| {
        let mut woke = vec![];
        for m in &mut org.sessions {
            if ready(m) {
                if m.snooze_notice == Some(true) && m.live {
                    woke.push((m.host.clone(), m.pane.clone(), name_for(m, &hosts), m.note.clone()));
                }
                m.snoozed_until = None;
                m.snooze_notice = None;
            }
        }
        org.sessions.retain(|m| !m.is_blank());
        renumber_pins(org);
        Ok(woke)
    });
    let Ok(woke) = woke else { return false };
    for (host, pane, title, note) in woke {
        let body = note.map(|n| n.lines().next().unwrap_or_default().to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Back from snooze".to_owned());
        emit(Event::Notice { host, pane, title, body });
    }
    true
}

/// One look at the clock: snoozes that are over, the marks matched again (so one whose session
/// has been gone a month is forgotten even when nothing else happens), messages due, idle agents
/// and finished sessions.
pub fn tick(now: u64) {
    wake_due(now);
    let hosts = registry().overview();
    if reconcile(&hosts) {
        emit(Event::Changed);
    }
    auto::follow_sends(&hosts);
    auto::send_due(now, &hosts);
    auto::check_agents(now, &hosts);
}

/// Starts the hub's clock for all that (once).
pub fn start() {
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    use crate::platform::rt::{sleep, spawn};
    spawn(async {
        loop {
            sleep(TICK_MS).await;
            tick(now_ms());
        }
    });
}

#[path = "organize_auto.rs"]
pub mod auto;

#[cfg(test)]
#[path = "organize_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "organize_auto_tests.rs"]
mod auto_tests;
