//! A model, effort or mode picked in the app while the agent can't take it: working, or asking
//! something on screen. Claude Code, Codex and Antigravity take these through keys typed into
//! their screen, which only works while they're free, so Inyon keeps the pick and makes it once
//! the agent is free, the last pick of each kind winning. It shows as pending on the session's
//! controls until then, on every device. A pick that fails keeps its reason, and nothing more is
//! tried for that pane until it's picked again or dropped.
//! Kept in the `chosen-controls` record, so a restart of the hub loses none.

use std::cell::RefCell;

use indexmap::IndexMap;
use inyon_protocol::{AgentKind, AgentState, ChosenControls};
use serde_json::Value;

use crate::hosts::http_error::HubResult;
use crate::hosts::registry::registry;
use crate::platform::store::kv;
use crate::re;

use super::watcher::watchers;

pub const KEY: &str = "chosen-controls";

thread_local! {
    static CHOSEN: RefCell<Option<IndexMap<String, ChosenControls>>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Model,
    Effort,
    Mode,
}

/// The order picks are made in: the mode first, then the model, whose level the effort sets.
const ORDER: [Pick; 3] = [Pick::Mode, Pick::Model, Pick::Effort];

fn key(host_id: &str, pane_id: &str) -> String {
    format!("{host_id}\u{1}{pane_id}")
}

fn with<T>(f: impl FnOnce(&mut IndexMap<String, ChosenControls>) -> T) -> T {
    CHOSEN.with(|c| {
        let mut c = c.borrow_mut();
        let all = c.get_or_insert_with(|| {
            let mut m = IndexMap::new();
            if let Some(Value::Object(saved)) = kv::get_value(KEY) {
                for (k, v) in saved {
                    if let Ok(chosen) = serde_json::from_value::<ChosenControls>(v) {
                        if !empty(&chosen) {
                            m.insert(k, chosen);
                        }
                    }
                }
            }
            m
        });
        f(all)
    })
}

fn empty(c: &ChosenControls) -> bool {
    c.model.is_none() && c.effort.is_none() && c.mode.is_none()
}

fn slot(c: &mut ChosenControls, pick: Pick) -> &mut Option<String> {
    match pick {
        Pick::Model => &mut c.model,
        Pick::Effort => &mut c.effort,
        Pick::Mode => &mut c.mode,
    }
}

/// Saves, and tells everyone watching the pane.
fn changed(host_id: &str, pane_id: &str) {
    with(|all| all.retain(|_, c| !empty(c)));
    let v = with(|all| serde_json::to_value(&*all).unwrap_or(Value::Null));
    if let Err(e) = kv::set_value(KEY, &v) {
        crate::platform::console::error(&format!("couldn't keep the picked controls: {e}"));
    }
    if let Some(w) = watchers().peek(host_id, pane_id) {
        w.schedule_info();
    }
}

/// A setting the app sent as a pick: `/model <model>` or `/effort <level>`.
pub fn from_setting(text: &str) -> Option<(Pick, String)> {
    let c = re!(r"^/(model|effort)\s+(\S.*)$").captures(text.trim())?;
    Some((if &c[1] == "model" { Pick::Model } else { Pick::Effort }, c[2].trim().to_owned()))
}

/// The agents whose picks are typed into their screen, so they wait until it's free.
fn waits(kind: AgentKind) -> bool {
    matches!(kind, AgentKind::Claude | AgentKind::Codex | AgentKind::Antigravity)
}

/// Whether the pane's agent can take a pick now: idle, with nothing in the background and
/// nothing asked on screen.
fn takes_now(host_id: &str, pane_id: &str) -> bool {
    let agent = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent);
    agent.is_some_and(|a| a.state == AgentState::Idle && a.waiting_for.is_none() && !inyon_protocol::is_busy(a.state, a.background.as_deref()))
        && watchers().peek(host_id, pane_id).is_none_or(|w| w.screen().prompt.is_none())
}

/// Keeps the pick for later when the pane's agent can't take it now, or when picks are already
/// waiting (so they're made in turn). True when it was kept; false means make it now.
pub fn hold_if_busy(host_id: &str, pane_id: &str, pick: Pick, value: &str) -> HubResult<bool> {
    let Some(kind) = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent).map(|a| a.kind) else { return Ok(false) };
    if !waits(kind) || (takes_now(host_id, pane_id) && !pending(host_id, pane_id)) {
        return Ok(false);
    }
    with(|all| {
        let c = all.entry(key(host_id, pane_id)).or_default();
        *slot(c, pick) = Some(value.to_owned());
        c.error = None;
    });
    changed(host_id, pane_id);
    super::held::start();
    Ok(true)
}

/// What's waiting for the pane, if anything.
pub fn get(host_id: &str, pane_id: &str) -> Option<ChosenControls> {
    with(|all| all.get(&key(host_id, pane_id)).cloned())
}

/// Drops what's waiting for the pane.
pub fn drop_all(host_id: &str, pane_id: &str) {
    if with(|all| all.shift_remove(&key(host_id, pane_id))).is_some() {
        changed(host_id, pane_id);
    }
}

/// Picks wait for the pane, and none has failed.
pub(crate) fn pending(host_id: &str, pane_id: &str) -> bool {
    get(host_id, pane_id).is_some_and(|c| c.error.is_none())
}

/// Panes with picks to make.
pub(crate) fn ready() -> Vec<(String, String)> {
    with(|all| all.iter().filter(|(_, c)| c.error.is_none()).filter_map(|(k, _)| k.split_once('\u{1}').map(|(h, p)| (h.to_owned(), p.to_owned()))).collect())
}

/// Makes the pane's picks in turn. Each goes once made; the first that fails keeps its reason
/// and stops the rest.
pub(crate) async fn apply(host_id: &str, pane_id: &str) {
    for pick in ORDER {
        let Some(value) = get(host_id, pane_id).and_then(|mut c| slot(&mut c, pick).clone()) else { continue };
        let r = match pick {
            Pick::Mode => super::actions::set_mode(host_id, pane_id, &value).await,
            Pick::Model => super::actions::send_message(host_id, pane_id, &format!("/model {value}"), false, None, true, false).await,
            Pick::Effort => super::actions::send_message(host_id, pane_id, &format!("/effort {value}"), false, None, true, false).await,
        };
        with(|all| {
            if let Some(c) = all.get_mut(&key(host_id, pane_id)) {
                match &r {
                    // a newer pick of the same kind came in meanwhile: that one still goes
                    Ok(()) => {
                        if slot(c, pick).as_deref() == Some(value.as_str()) {
                            *slot(c, pick) = None;
                        }
                    }
                    Err(e) => c.error = Some(e.message.clone()),
                }
            }
        });
        changed(host_id, pane_id);
        if r.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "chosen_tests.rs"]
mod tests;
