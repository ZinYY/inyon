//! Port of hosts/names.ts.
//!
//! Names you gave agent sessions in Inyon, keyed by machine and conversation id, so a name
//! follows the conversation through resumes and pane changes. `was` keeps the agent's own title
//! from before the first rename, since renaming also renames the conversation inside the agent.
//!
//! Agents that keep their own title (Claude, Codex, agy) are the record: a rename waits here as
//! `pending` until the agent has taken it, and from then on the agent's title is what shows, on
//! this hub, in the phone app and in the agent's resume picker alike. A rename made elsewhere
//! (another app, /rename at the keyboard) wins the same way. Other agents keep the name here.

use crate::re;
use std::cell::RefCell;

use indexmap::IndexMap;
use inyon_protocol::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform::store::kv;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear: Option<bool>,
}

thread_local! {
    static NAMES: RefCell<Option<IndexMap<String, Entry>>> = const { RefCell::new(None) };
}

/// The names, read from the store the first time (the TS reads them when the module loads).
fn with<R>(f: impl FnOnce(&mut IndexMap<String, Entry>) -> R) -> R {
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        let names = n.get_or_insert_with(|| {
            let saved: IndexMap<String, Value> = kv::get("names").unwrap_or_default();
            saved
                .into_iter()
                .filter_map(|(k, v)| match v {
                    // names saved before renames waited for the agent were just the name
                    Value::String(name) => Some((k, Entry { name, was: None, pending: None, clear: None })),
                    v => serde_json::from_value::<Entry>(v).ok().map(|e| (k, e)),
                })
                .collect()
        });
        f(names)
    })
}

fn k(host: &str, session: &str) -> String {
    format!("{host} {session}")
}

fn save(names: &IndexMap<String, Entry>) {
    let _ = kv::set("names", names);
}

/// Agents with a title of their own that /rename sets.
pub fn owns_title(kind: Option<AgentKind>) -> bool {
    matches!(kind, Some(AgentKind::Claude | AgentKind::Codex | AgentKind::Antigravity))
}

/// The name to show over the agent's own title, if any.
pub fn session_name(host: &str, session: Option<&str>, kind: Option<AgentKind>) -> Option<String> {
    let e = with(|n| session.and_then(|s| n.get(&k(host, s)).cloned()))?;
    (!owns_title(kind) || e.pending == Some(true)).then_some(e.name)
}

/// The name Inyon last gave a session, taken by the agent or not: what to call it while it isn't running.
pub fn last_name(host: &str, session: &str) -> Option<String> {
    with(|n| n.get(&k(host, session)).map(|e| e.name.clone())).filter(|n| !n.is_empty())
}

/// The agent's own title from before Inyon renamed it, if Inyon did.
pub fn original_title(host: &str, session: &str) -> Option<String> {
    with(|n| n.get(&k(host, session)).and_then(|e| e.was.clone()))
}

/// A rename the agent hasn't taken yet.
pub fn pending_name(host: &str, session: Option<&str>) -> Option<Entry> {
    with(|n| session.and_then(|s| n.get(&k(host, s)).cloned())).filter(|e| e.pending == Some(true))
}

/// Names a session. For an agent with its own title the name waits until the agent has it; an
/// empty name goes back to the title the agent had made up (`was`), and is then forgotten.
pub fn set_session_name(host: &str, session: &str, name: &str, was: Option<&str>, kind: Option<AgentKind>) {
    with(|n| {
        let key = k(host, session);
        let before = n.get(&key).and_then(|e| e.was.clone()).or_else(|| was.map(str::to_owned));
        if owns_title(kind) {
            if !name.is_empty() {
                n.insert(key, Entry { name: name.into(), was: before, pending: Some(true), clear: None });
            } else if let Some(before) = before {
                n.insert(key, Entry { name: before.clone(), was: Some(before), pending: Some(true), clear: Some(true) });
            } else {
                n.shift_remove(&key);
            }
        } else if !name.is_empty() {
            n.insert(key, Entry { name: name.into(), was: before, pending: None, clear: None });
        } else {
            n.shift_remove(&key);
        }
        save(n);
    })
}

/// The agent now shows the pending name: its title is the record from here on.
pub fn name_taken(host: &str, session: &str) {
    with(|n| {
        let key = k(host, session);
        let Some(e) = n.get(&key).cloned() else { return };
        if e.pending != Some(true) {
            return;
        }
        if e.clear == Some(true) {
            n.shift_remove(&key);
        } else {
            n.insert(key, Entry { name: e.name, was: e.was, pending: None, clear: None });
        }
        save(n);
    })
}

/// The title an agent shows, without the activity mark Claude puts in front ("✳ ", "⠂ "):
/// `title.replace(/^[^\p{L}\p{N}"'(]+\s/u, '').trim()`.
pub fn bare_title(title: Option<&str>) -> String {
    use crate::js::JsStr;
    let t = title.unwrap_or("");
    re!(r#"^[^\p{L}\p{N}"'(]+\s"#, "u").replace(t, "").jtrim().to_owned()
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
