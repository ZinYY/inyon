//! What the hub does for organised sessions on its own (from 0.24): messages sent at a set time,
//! idle agents put to sleep, sessions ended once their work is done, and the prompts kept to send
//! again. All of it lives in the `organize` record next to the marks, so it survives a restart.
//!
//! Each automatic step fires once. A message sent later leaves the record (written down) before it
//! is handed to the held queue, and a session closed when done loses its flag before it's ended,
//! so a hub that stops halfway through doesn't do it again when it starts. The hand-offs go through
//! [`set_actions`] in tests.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use inyon_protocol::{
    is_busy, js_len, project_of, sleep_minutes, AgentState, DelayedSend, HostState, HostStatus, Organize, PaneInfo, SavedPrompt, SessionInfo, SleepRule, MAX_PROMPTS, MAX_SENDS,
    PROMPT_LEN, SEND_LEN, SLEEP_ALL, SLEEP_MAX, SLEEP_MIN,
};
use serde_json::{Map, Value};

use super::{bool_field, change, change_or_keep, conv_of, given, new_id, object, ref_field, with, Given, MAX_SNOOZE_MS};
use crate::clock::now_ms;
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;

/// A session closed when done is ended once its agent has been idle this long in a row.
pub const CLOSE_SETTLE_MS: u64 = 5_000;
/// A session that couldn't be ended is tried again after this long.
pub const CLOSE_RETRY_MS: u64 = 60_000;

/// What the hub hands on to the rest of Inyon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// a message sent later, into the pane's held queue
    Hold { host: String, pane: String, text: String },
    /// an idle agent, exited to its shell (Exit to shell)
    Sleep { host: String, pane: String },
    /// a session done with its work, ended (End session)
    End { host: String, pane: String },
}

type Actions = Rc<dyn Fn(&Act) -> HubResult<()>>;

thread_local! {
    static ACTIONS: RefCell<Option<Actions>> = const { RefCell::new(None) };
    /// when each agent was first seen idle in a row, by "host pane"
    static IDLE_SINCE: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// agents already put to sleep (or tried) in this idle stretch
    static SLEPT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// sessions that couldn't be ended, by mark id: not tried again before this time
    static CLOSE_AFTER: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// Replaces the hand-offs (tests); None goes back to the real ones.
pub fn set_actions(f: Option<Actions>) {
    ACTIONS.with(|a| *a.borrow_mut() = f);
    IDLE_SINCE.with(|m| m.borrow_mut().clear());
    SLEPT.with(|s| s.borrow_mut().clear());
    CLOSE_AFTER.with(|m| m.borrow_mut().clear());
}

fn act(a: Act) -> HubResult<()> {
    if let Some(f) = ACTIONS.with(|x| x.borrow().clone()) {
        return f(&a);
    }
    use crate::platform::rt::spawn;
    match a {
        Act::Hold { host, pane, text } => crate::claude::held::hold(&host, &pane, &text).map(|_| ()),
        Act::Sleep { host, pane } => {
            spawn(async move {
                if let Err(e) = crate::claude::actions::exit_agent(&host, &pane, false).await {
                    crate::platform::console::error(&format!("couldn't put the agent in {host} {pane} to sleep: {}", e.message));
                }
            });
            Ok(())
        }
        Act::End { host, pane } => {
            spawn(async move {
                if let Err(e) = crate::claude::actions::end_session(&host, &pane).await {
                    crate::platform::console::error(&format!("couldn't end the session in {host} {pane} once done: {}", e.message));
                    rearm_close(&host, &pane, now_ms());
                }
            });
            Ok(())
        }
    }
}

/// An agent in a pane: another agent started in the same pane is another key, so it doesn't
/// take over the first one's idle time.
fn key(host: &str, pane: &PaneInfo) -> String {
    let (pid, conv) = pane.agent.as_ref().map_or((0, ""), |a| (a.pid, a.session_id.as_deref().unwrap_or_default()));
    format!("{host}\u{1}{}\u{1}{pid}\u{1}{conv}", pane.id)
}

/// The panes of the machines that are online: what one that isn't shows is only what it last
/// reported, nothing to act on.
fn panes(hosts: &[HostState]) -> impl Iterator<Item = (&HostState, &SessionInfo, &PaneInfo)> {
    hosts
        .iter()
        .filter(|h| h.status == HostStatus::Online)
        .flat_map(|h| h.sessions.iter().flat_map(move |s| s.windows.iter().flat_map(move |w| w.panes.iter().map(move |p| (h, s, p)))))
}

/// Idle: done with its turn, nothing in the background, not asking anything.
fn idle(p: &PaneInfo) -> bool {
    p.agent.as_ref().is_some_and(|a| a.state == AgentState::Idle && a.waiting_for.is_none() && !is_busy(a.state, a.background.as_deref()))
}

fn text_field(b: &Map<String, Value>, required: bool, max: usize) -> HubResult<Option<String>> {
    match given(b, "text") {
        Given::Absent if !required => Ok(None),
        Given::Some(Value::String(s)) if !s.trim().is_empty() => {
            if js_len(s) > max {
                return Err(http(400, format!("A message can be at most {max} characters")));
            }
            Ok(Some(s.trim_end().to_owned()))
        }
        _ => Err(http(400, "Message is empty")),
    }
}

fn at_field(b: &Map<String, Value>, required: bool, now: u64) -> HubResult<Option<u64>> {
    match given(b, "at") {
        Given::Absent if !required => Ok(None),
        Given::Some(v) => {
            let at = v.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| http(400, "at must be a time (unix ms)"))? as u64;
            if at <= now {
                return Err(http(400, "Pick a time in the future to send it"));
            }
            if at - now > MAX_SNOOZE_MS {
                return Err(http(400, "A message can wait a year at most"));
            }
            Ok(Some(at))
        }
        _ => Err(http(400, "Pick a time to send it")),
    }
}

// ---------- messages sent later ----------

/// Keeps a message to send to the pane's agent at `at`.
pub fn add_send(host_id: &str, pane_id: &str, b: &Value, now: u64) -> HubResult<DelayedSend> {
    let b = object(b)?;
    let (text, at) = (text_field(b, true, SEND_LEN)?.unwrap_or_default(), at_field(b, true, now)?.unwrap_or_default());
    let conv = match given(b, "conv") {
        Given::Absent | Given::Null => None,
        Given::Some(Value::String(c)) if !c.is_empty() && c.len() <= 200 => Some(c.clone()),
        Given::Some(_) => return Err(http(400, "conv must be a conversation id")),
    };
    let f = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    if f.pane.agent.is_none() {
        return Err(http(409, "No agent is running in this pane"));
    }
    change(|org| {
        let sends = org.sends.get_or_insert_with(Vec::new);
        if sends.len() >= MAX_SENDS {
            return Err(http(409, format!("{MAX_SENDS} messages are waiting to be sent already. Send or cancel some first.")));
        }
        let s = DelayedSend {
            id: new_id("d"),
            host: host_id.to_owned(),
            pane: pane_id.to_owned(),
            // one put back (Undo after a cancel) keeps the conversation it was for
            conv: conv.or_else(|| conv_of(&f.pane).map(str::to_owned)),
            tmux_id: Some(f.session.id.clone()),
            text,
            at,
            error: None,
        };
        sends.push(s.clone());
        Ok(s)
    })
}

/// New text or time for a message waiting to go; one that failed can go again.
pub fn update_send(id: &str, b: &Value, now: u64) -> HubResult<DelayedSend> {
    let b = object(b)?;
    let (text, at) = (text_field(b, false, SEND_LEN)?, at_field(b, false, now)?);
    change(|org| {
        let s = org.sends.get_or_insert_with(Vec::new).iter_mut().find(|s| s.id == id).ok_or_else(|| http(404, "That message isn't waiting any more"))?;
        if let Some(t) = text {
            s.text = t;
        }
        if let Some(a) = at {
            s.at = a;
        }
        s.error = None;
        Ok(s.clone())
    })
}

/// Cancels a message waiting to go (or dismisses one that failed).
pub fn remove_send(id: &str) -> HubResult<()> {
    change(|org| {
        let sends = org.sends.get_or_insert_with(Vec::new);
        let n = sends.len();
        sends.retain(|s| s.id != id);
        if sends.len() == n {
            return Err(http(404, "That message isn't waiting any more"));
        }
        tidy(org);
        Ok(())
    })
}

/// The pane a message goes to now: its conversation's wherever that is. One kept without a
/// conversation goes to its pane, if that's still in the same tmux session. One whose
/// conversation is nowhere goes nowhere: another conversation in its old pane isn't the one it
/// was written for.
fn target(s: &DelayedSend, hosts: &[HostState]) -> Option<String> {
    let on_host = || panes(hosts).filter(|(h, _, _)| h.config.id == s.host);
    if let Some(conv) = s.conv.as_deref() {
        return on_host().find(|(_, _, p)| conv_of(p) == Some(conv)).map(|(_, _, p)| p.id.clone());
    }
    on_host().find(|(_, t, p)| p.id == s.pane && p.agent.is_some() && s.tmux_id.as_deref().is_none_or(|id| id == t.id)).map(|(_, _, p)| p.id.clone())
}

/// Hands the messages whose time has come to their panes' held queues, once each. Whether any went.
pub fn send_due(now: u64, hosts: &[HostState]) -> bool {
    // one for a machine that isn't online waits for it to be back
    let away = |host: &str| hosts.iter().any(|h| h.config.id == host && h.status != HostStatus::Online);
    let due: Vec<DelayedSend> = with(|st| st.org.sends.iter().flatten().filter(|s| s.at <= now && s.error.is_none() && !away(&s.host)).cloned().collect::<Vec<_>>());
    if due.is_empty() {
        return false;
    }
    for s in due {
        let _ = deliver(&s.id, hosts);
    }
    true
}

/// `POST /api/hosts/:id/sends/:did/now`: a waiting message (or one that failed) goes now, in one
/// step, so it can't be both held and still waiting.
pub fn send_now(id: &str) -> HubResult<String> {
    deliver(id, &registry().overview())
}

/// Hands a waiting message to the held queue of the pane it goes to now, returning that pane. It
/// leaves the record (written down) first, so a hub that stops after this never sends it twice;
/// when it can't go, it's back on the list with the reason.
fn deliver(id: &str, hosts: &[HostState]) -> HubResult<String> {
    let s = with(|st| st.org.sends.iter().flatten().find(|s| s.id == id).cloned()).ok_or_else(|| http(404, "That message isn't waiting any more"))?;
    let to = target(&s, hosts);
    change(|org| {
        let sends = org.sends.get_or_insert_with(Vec::new);
        let i = sends.iter().position(|x| x.id == s.id).ok_or_else(|| http(404, "That message isn't waiting any more"))?;
        match &to {
            Some(_) => {
                sends.remove(i);
            }
            None => sends[i].error = Some(GONE.into()),
        }
        tidy(org);
        Ok(())
    })?;
    let Some(pane) = to else { return Err(http(409, GONE)) };
    if let Err(e) = act(Act::Hold { host: s.host.clone(), pane: pane.clone(), text: s.text.clone() }) {
        // kept even when it can't be written down now, so it can still be retried or dismissed
        let back = DelayedSend { pane: pane.clone(), error: Some(e.message.clone()), ..s };
        if let Err(w) = change_or_keep(|org| org.sends.get_or_insert_with(Vec::new).push(back)) {
            crate::platform::console::error(&format!("a message that couldn't be sent is kept, but couldn't be written down: {}", w.message));
        }
        return Err(e);
    }
    Ok(pane)
}

const GONE: &str = "The session isn't there any more";

/// Moves each waiting message to the pane its conversation is in now, so the chat there shows it.
/// Whether any moved.
pub fn follow_sends(hosts: &[HostState]) -> bool {
    let moves: Vec<(String, String, String)> = with(|st| {
        st.org
            .sends
            .iter()
            .flatten()
            .filter_map(|s| {
                let conv = s.conv.as_deref()?;
                let (_, t, p) = panes(hosts).find(|(h, _, p)| h.config.id == s.host && conv_of(p) == Some(conv))?;
                (p.id != s.pane).then(|| (s.id.clone(), p.id.clone(), t.id.clone()))
            })
            .collect()
    });
    if moves.is_empty() {
        return false;
    }
    change(|org| {
        for s in org.sends.iter_mut().flatten() {
            if let Some((_, pane, tmux)) = moves.iter().find(|(id, _, _)| *id == s.id) {
                s.pane = pane.clone();
                s.tmux_id = Some(tmux.clone());
            }
        }
        Ok(())
    })
    .is_ok()
}

// ---------- idle agents and finished sessions ----------

/// `PUT /api/organize/sleep`: `{ scope: "all" | project id, minutes: N | null }` (null turns it off).
pub fn set_sleep(b: &Value) -> HubResult<Vec<SleepRule>> {
    let b = object(b)?;
    let scope = match given(b, "scope") {
        Given::Absent | Given::Null => SLEEP_ALL.to_owned(),
        Given::Some(Value::String(s)) => s.clone(),
        Given::Some(_) => return Err(http(400, "scope must be \"all\" or a project id")),
    };
    let minutes = match given(b, "minutes") {
        Given::Absent | Given::Null => None,
        Given::Some(v) => {
            let m = v.as_f64().filter(|m| m.fract() == 0.0 && (SLEEP_MIN as f64..=SLEEP_MAX as f64).contains(m));
            Some(m.ok_or_else(|| http(400, format!("Pick between {SLEEP_MIN} minutes and {} hours", SLEEP_MAX / 60)))? as u32)
        }
    };
    change(|org| {
        if scope != SLEEP_ALL && !org.projects.iter().any(|p| p.id == scope) {
            return Err(http(404, "There's no such project"));
        }
        let rules = org.sleep.get_or_insert_with(Vec::new);
        rules.retain(|r| r.scope != scope);
        if let Some(minutes) = minutes {
            rules.push(SleepRule { scope, minutes });
        }
        let out = rules.clone();
        tidy(org);
        Ok(out)
    })
}

/// One look at every agent: puts the ones idle past their project's (or everyone's) rule to sleep,
/// and ends the sessions that were to close once done and now are.
pub fn check_agents(now: u64, hosts: &[HostState]) {
    let org = with(|st| (!st.newer).then(|| st.org.clone()));
    let Some(org) = org else { return };
    let mut seen: HashSet<String> = HashSet::new();
    let mut finished: Vec<(String, String, String)> = vec![];
    let mut worked: Vec<String> = vec![];
    for (h, _, p) in panes(hosts) {
        let Some(a) = &p.agent else { continue };
        let k = key(&h.config.id, p);
        seen.insert(k.clone());
        let since = IDLE_SINCE.with(|m| {
            let mut m = m.borrow_mut();
            if idle(p) {
                Some(*m.entry(k.clone()).or_insert(now))
            } else {
                m.remove(&k);
                None
            }
        });
        if since.is_none() {
            SLEPT.with(|s| s.borrow_mut().remove(&k));
        }
        let mark = org.sessions.iter().find(|m| m.live && m.host == h.config.id && m.pane == p.id);
        if let Some(m) = mark.filter(|m| m.close_when_done == Some(true)) {
            match since {
                None if m.close_saw_work != Some(true) => worked.push(m.id.clone()),
                Some(t)
                    if m.close_saw_work == Some(true) && now.saturating_sub(t) >= CLOSE_SETTLE_MS && CLOSE_AFTER.with(|c| c.borrow().get(&m.id).is_none_or(|at| now >= *at)) =>
                {
                    finished.push((m.id.clone(), h.config.id.clone(), p.id.clone()))
                }
                _ => {}
            }
            // it ends anyway: no sleep for it
            continue;
        }
        let Some(t) = since else { continue };
        let Some(minutes) = sleep_minutes(&org, project_of(&org, &h.config.id, &p.cwd, mark)) else { continue };
        // a pinned one stays awake, and so does one without a conversation to resume, one with a
        // message waiting for it, and the companion
        // (a message goes to its conversation wherever that is now)
        let waiting = !crate::claude::held::list(&h.config.id, &p.id).is_empty()
            || org.sends.iter().flatten().any(|s| s.host == h.config.id && (s.pane == p.id || (s.conv.is_some() && s.conv == a.session_id)));
        if mark.is_some_and(|m| m.pinned.is_some()) || a.session_id.as_deref().is_none_or(str::is_empty) || p.companion == Some(true) || waiting {
            continue;
        }
        if now.saturating_sub(t) >= u64::from(minutes) * 60_000 && SLEPT.with(|s| s.borrow_mut().insert(k)) {
            let _ = act(Act::Sleep { host: h.config.id.clone(), pane: p.id.clone() });
        }
    }
    IDLE_SINCE.with(|m| m.borrow_mut().retain(|k, _| seen.contains(k)));
    SLEPT.with(|s| s.borrow_mut().retain(|k| seen.contains(k)));
    if !worked.is_empty() {
        let _ = change(|org| {
            for m in org.sessions.iter_mut().filter(|m| worked.contains(&m.id)) {
                m.close_saw_work = Some(true);
            }
            Ok(())
        });
    }
    for (id, host, pane) in finished {
        // the flag goes first (written down), so it's ended once even if the hub stops right after
        let took = change(|org| {
            let Some(i) = org.sessions.iter().position(|m| m.id == id && m.close_when_done == Some(true)) else { return Ok(false) };
            org.sessions[i].close_when_done = None;
            org.sessions[i].close_saw_work = None;
            if org.sessions[i].is_blank() {
                org.sessions.remove(i);
                super::renumber_pins(org);
            }
            Ok(true)
        });
        if took == Ok(true) && act(Act::End { host: host.clone(), pane: pane.clone() }).is_err() {
            rearm_close(&host, &pane, now);
        }
    }
}

/// A session that couldn't be ended once done is to close again, tried after a while: the flag
/// went before the attempt, so it comes back now.
fn rearm_close(host: &str, pane: &str, now: u64) {
    let rearmed = super::mark_pane(host, pane, &super::SessionPatch { close_when_done: Some(true), ..Default::default() }).and_then(|m| {
        let Some(m) = m else { return Ok(None) };
        change(|org| {
            if let Some(x) = org.sessions.iter_mut().find(|x| x.id == m.id) {
                x.close_saw_work = Some(true);
            }
            Ok(Some(m.id.clone()))
        })
    });
    match rearmed {
        Ok(Some(id)) => CLOSE_AFTER.with(|c| c.borrow_mut().insert(id, now + CLOSE_RETRY_MS)),
        Ok(None) => None,
        Err(e) => {
            crate::platform::console::error(&format!("couldn't keep {host} {pane} to close when done: {}", e.message));
            None
        }
    };
}

// ---------- saved prompts ----------

fn set_prompt(org: &Organize, q: &mut SavedPrompt, b: &Map<String, Value>, new: bool) -> HubResult<()> {
    if let Some(n) = super::name_field(b, new, "prompt")? {
        q.name = n;
    }
    if let Some(t) = text_field(b, new, PROMPT_LEN)? {
        q.text = t;
    }
    if let Some(p) = ref_field(b, "project")? {
        if p.as_ref().is_some_and(|p| !org.projects.iter().any(|x| &x.id == p)) {
            return Err(http(404, "There's no such project"));
        }
        q.project = p;
    }
    if let Some(s) = bool_field(b, "starred")? {
        q.starred = s.then_some(true);
    }
    Ok(())
}

pub fn add_prompt(b: &Value) -> HubResult<SavedPrompt> {
    let b = object(b)?;
    change(|org| {
        if org.prompts.as_ref().is_some_and(|l| l.len() >= MAX_PROMPTS) {
            return Err(http(409, format!("Inyon keeps {MAX_PROMPTS} prompts at most")));
        }
        let mut q = SavedPrompt { id: new_id("q"), name: String::new(), text: String::new(), project: None, starred: None };
        set_prompt(org, &mut q, b, true)?;
        org.prompts.get_or_insert_with(Vec::new).push(q.clone());
        Ok(q)
    })
}

pub fn update_prompt(id: &str, b: &Value) -> HubResult<SavedPrompt> {
    let b = object(b)?;
    change(|org| {
        let mut q = org.prompts.iter().flatten().find(|q| q.id == id).cloned().ok_or_else(|| http(404, "There's no such prompt"))?;
        set_prompt(org, &mut q, b, false)?;
        if let Some(slot) = org.prompts.iter_mut().flatten().find(|x| x.id == id) {
            *slot = q.clone();
        }
        Ok(q)
    })
}

pub fn remove_prompt(id: &str) -> HubResult<()> {
    change(|org| {
        let prompts = org.prompts.get_or_insert_with(Vec::new);
        let n = prompts.len();
        prompts.retain(|q| q.id != id);
        if prompts.len() == n {
            return Err(http(404, "There's no such prompt"));
        }
        tidy(org);
        Ok(())
    })
}

/// A removed project's prompts are kept for every session, and its sleep rule goes.
pub(super) fn project_removed(org: &mut Organize, id: &str) {
    for q in org.prompts.iter_mut().flatten() {
        if q.project.as_deref() == Some(id) {
            q.project = None;
        }
    }
    if let Some(rules) = &mut org.sleep {
        rules.retain(|r| r.scope != id);
    }
    tidy(org);
}

/// Empty lists aren't written.
pub(super) fn tidy(org: &mut Organize) {
    if org.sends.as_ref().is_some_and(Vec::is_empty) {
        org.sends = None;
    }
    if org.prompts.as_ref().is_some_and(Vec::is_empty) {
        org.prompts = None;
    }
    if org.sleep.as_ref().is_some_and(Vec::is_empty) {
        org.sleep = None;
    }
}

/// What a v1 overview carries: everything but the saved prompts, a library that can be long and
/// rarely changes; GET /api/organize and the `/ws2` stream have them.
pub fn for_overview(mut org: Organize) -> Organize {
    org.prompts = None;
    org
}
