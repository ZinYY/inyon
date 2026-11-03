//! Messages Inyon holds for a pane and types in once its agent is free: a queue the hub owns, so
//! it can still be edited, reordered or dropped from any device, which an agent's own queue
//! can't. The first message goes when the agent has been idle for a moment with nothing open on
//! screen; a send that fails keeps its message at the front with the reason, and holding stops
//! there until it's edited, sent now or dropped, so a broken pane isn't typed into again and again.
//! Kept in the `held-prompts` record, so a restart of the hub loses none.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use inyon_protocol::{AgentState, HeldPrompt};
use serde_json::Value;

use crate::clock::now_ms;
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::js::text::JsStr;
use crate::platform::rt::{sleep, spawn};
use crate::platform::store::kv;

use super::watcher::watchers;

pub const KEY: &str = "held-prompts";
/// Messages held for one pane at most.
pub const MAX: usize = 50;
/// How long the agent has to have been idle before the next message goes.
pub const SETTLE_MS: u64 = 1500;
const TICK_MS: u64 = 500;

thread_local! {
    static HELD: RefCell<Option<IndexMap<String, Vec<HeldPrompt>>>> = const { RefCell::new(None) };
    /// panes whose first message is being typed now
    static SENDING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// when each pane was first seen idle in a row
    static IDLE_SINCE: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

fn key(host_id: &str, pane_id: &str) -> String {
    format!("{host_id}\u{1}{pane_id}")
}

fn with<T>(f: impl FnOnce(&mut IndexMap<String, Vec<HeldPrompt>>) -> T) -> T {
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        let all = h.get_or_insert_with(|| {
            let mut m = IndexMap::new();
            if let Some(Value::Object(saved)) = kv::get_value(KEY) {
                for (k, v) in saved {
                    if let Ok(list) = serde_json::from_value::<Vec<HeldPrompt>>(v) {
                        if !list.is_empty() {
                            m.insert(k, list);
                        }
                    }
                }
            }
            m
        });
        f(all)
    })
}

fn save() {
    let v = with(|all| serde_json::to_value(&*all).unwrap_or(Value::Null));
    if let Err(e) = kv::set_value(KEY, &v) {
        crate::platform::console::error(&format!("couldn't keep the held messages: {e}"));
    }
}

/// Saves, and tells everyone watching the pane.
fn changed(host_id: &str, pane_id: &str) {
    with(|all| all.retain(|_, l| !l.is_empty()));
    save();
    if let Some(w) = watchers().peek(host_id, pane_id) {
        w.schedule_info();
    }
}

/// The messages held for a pane, in the order they go.
pub fn list(host_id: &str, pane_id: &str) -> Vec<HeldPrompt> {
    with(|all| all.get(&key(host_id, pane_id)).cloned().unwrap_or_default())
}

fn find(host_id: &str, pane_id: &str, id: &str) -> HubResult<usize> {
    list(host_id, pane_id).iter().position(|p| p.id == id).ok_or_else(|| http(404, "That message isn't held any more"))
}

/// Holds a message for the pane's agent, last in line.
pub fn hold(host_id: &str, pane_id: &str, text: &str) -> HubResult<HeldPrompt> {
    let found = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    if found.pane.agent.is_none() {
        return Err(http(409, "No agent is running in this pane"));
    }
    if text.is_blank() {
        return Err(http(400, "Message is empty"));
    }
    if list(host_id, pane_id).len() >= MAX {
        return Err(http(409, format!("{MAX} messages are held for this session already. Send or drop some first.")));
    }
    let p = HeldPrompt { id: format!("held-{}", crate::ids::random_uuid()), text: text.to_owned(), at: now_ms(), error: None };
    with(|all| all.entry(key(host_id, pane_id)).or_default().push(p.clone()));
    changed(host_id, pane_id);
    start();
    Ok(p)
}

/// New text for a held message; it can go again if its last try failed.
pub fn edit(host_id: &str, pane_id: &str, id: &str, text: &str) -> HubResult<()> {
    if text.is_blank() {
        return Err(http(400, "Message is empty"));
    }
    let i = find(host_id, pane_id, id)?;
    if i == 0 && SENDING.with(|s| s.borrow().contains(&key(host_id, pane_id))) {
        return Err(http(409, "That message is being typed in now"));
    }
    with(|all| {
        if let Some(p) = all.get_mut(&key(host_id, pane_id)).and_then(|l| l.get_mut(i)) {
            p.text = text.to_owned();
            p.error = None;
        }
    });
    changed(host_id, pane_id);
    Ok(())
}

/// Drops a held message.
pub fn remove(host_id: &str, pane_id: &str, id: &str) -> HubResult<()> {
    let i = find(host_id, pane_id, id)?;
    if i == 0 && SENDING.with(|s| s.borrow().contains(&key(host_id, pane_id))) {
        return Err(http(409, "That message is being typed in now"));
    }
    with(|all| {
        if let Some(l) = all.get_mut(&key(host_id, pane_id)) {
            l.remove(i);
        }
    });
    changed(host_id, pane_id);
    Ok(())
}

/// Moves a held message to `to` (0 is next), clamped to the list.
pub fn move_to(host_id: &str, pane_id: &str, id: &str, to: usize) -> HubResult<()> {
    let i = find(host_id, pane_id, id)?;
    let k = key(host_id, pane_id);
    if SENDING.with(|s| s.borrow().contains(&k)) && (i == 0 || to == 0) {
        return Err(http(409, "The next message is being typed in now"));
    }
    with(|all| {
        if let Some(l) = all.get_mut(&k) {
            let p = l.remove(i);
            let to = to.min(l.len());
            l.insert(to, p);
        }
    });
    changed(host_id, pane_id);
    Ok(())
}

/// Sends a held message now, whatever the agent is doing (it may queue it itself).
pub async fn send_now(host_id: &str, pane_id: &str, id: &str) -> HubResult<()> {
    find(host_id, pane_id, id)?;
    move_to(host_id, pane_id, id, 0)?;
    send_first(host_id, pane_id).await
}

/// Types the pane's first held message in. Gone from the list once the agent took it; kept with
/// the reason when it didn't.
async fn send_first(host_id: &str, pane_id: &str) -> HubResult<()> {
    let k = key(host_id, pane_id);
    let Some(first) = list(host_id, pane_id).into_iter().next() else { return Ok(()) };
    if !SENDING.with(|s| s.borrow_mut().insert(k.clone())) {
        return Err(http(409, "The next message is being typed in now"));
    }
    let r = super::actions::send_message(host_id, pane_id, &first.text, false, Some(&first.id), false, false).await;
    SENDING.with(|s| s.borrow_mut().remove(&k));
    IDLE_SINCE.with(|m| m.borrow_mut().remove(&k));
    with(|all| {
        if let Some(l) = all.get_mut(&k) {
            if let Some(i) = l.iter().position(|p| p.id == first.id) {
                match &r {
                    Ok(()) => {
                        l.remove(i);
                    }
                    Err(e) => l[i].error = Some(e.message.clone()),
                }
            }
        }
    });
    changed(host_id, pane_id);
    r
}

/// Whether the pane's agent is free for the next message: running, idle with nothing in the
/// background, not asking anything, and for long enough.
fn free(host_id: &str, pane_id: &str, now: u64) -> bool {
    let k = key(host_id, pane_id);
    let agent = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent);
    let idle = agent.is_some_and(|a| a.state == AgentState::Idle && a.waiting_for.is_none() && !inyon_protocol::is_busy(a.state, a.background.as_deref()))
        && watchers().peek(host_id, pane_id).is_none_or(|w| w.screen().prompt.is_none());
    IDLE_SINCE.with(|m| {
        let mut m = m.borrow_mut();
        if !idle {
            m.remove(&k);
            return false;
        }
        let since = *m.entry(k).or_insert(now);
        now.saturating_sub(since) >= SETTLE_MS
    })
}

/// One look at every pane with held messages or picked controls: where the agent is free, the
/// controls picked meanwhile go first (see chosen), then the first message.
pub async fn tick(now: u64) {
    for (host_id, pane_id) in super::chosen::ready() {
        let k = key(&host_id, &pane_id);
        if SENDING.with(|s| s.borrow().contains(&k)) || !free(&host_id, &pane_id, now) {
            continue;
        }
        SENDING.with(|s| s.borrow_mut().insert(k.clone()));
        spawn(async move {
            super::chosen::apply(&host_id, &pane_id).await;
            SENDING.with(|s| s.borrow_mut().remove(&k));
            IDLE_SINCE.with(|m| m.borrow_mut().remove(&k));
        });
    }
    let ready: Vec<(String, String)> = with(|all| {
        all.iter().filter(|(_, l)| l.first().is_some_and(|p| p.error.is_none())).filter_map(|(k, _)| k.split_once('\u{1}').map(|(h, p)| (h.to_owned(), p.to_owned()))).collect()
    });
    for (host_id, pane_id) in ready {
        if SENDING.with(|s| s.borrow().contains(&key(&host_id, &pane_id))) || super::chosen::pending(&host_id, &pane_id) || !free(&host_id, &pane_id, now) {
            continue;
        }
        spawn(async move {
            let _ = send_first(&host_id, &pane_id).await;
        });
    }
}

/// Starts looking, once: at the hub's start for messages held before a restart, and on the first hold.
pub fn start() {
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    spawn(async {
        loop {
            if with(|all| !all.is_empty()) || !super::chosen::ready().is_empty() {
                tick(now_ms()).await;
            }
            sleep(TICK_MS).await;
        }
    });
}
