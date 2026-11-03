//! Pages an agent made but never added: every few minutes, while anyone could see them, each
//! machine's own `inyon pages find` looks under the folders its agents work in for Blueprint sites
//! (a `.inyon-page.json` next to an `index.html`) and adds those not on its list yet. A page taken
//! off the list stays off: the list remembers it under `removed`. The hub keeps which session's
//! folder each page was found under (`page-sessions`), so the app can show that session's name.

use std::collections::{BTreeMap, HashMap};

use inyon_core::features::pages_core::LOCAL_HOST;
use inyon_core::hosts::registry::registry;
use inyon_core::platform::rt::{sleep, spawn};
use inyon_core::platform::store::kv;
use inyon_protocol::{HostState, HostStatus};
use serde_json::{Map, Value};

/// How often the agents' folders are looked through, in ms.
pub const EVERY_MS: u64 = 5 * 60_000;
/// How long a machine whose inyon can't find pages (before 0.22.4) is left before it's asked again.
pub const RETRY_MS: u64 = 30 * 60_000;
/// The most folders sent to one machine at a time.
const DIRS_MAX: usize = 32;
/// The most pages whose session the hub keeps.
const SESSIONS_MAX: usize = 500;

/// The folders each kind of agent works in on a machine, from its sessions: agent panes only, with
/// an absolute folder (a Windows one too), each with the conversation working there when known.
pub fn agent_folders(state: &HostState) -> BTreeMap<String, BTreeMap<String, Option<String>>> {
    let mut out: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
    for pane in state.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes) {
        let Some(agent) = &pane.agent else { continue };
        let cwd = pane.cwd.trim();
        let absolute = cwd.starts_with('/') || cwd.as_bytes().get(1..3).is_some_and(|b| b[0] == b':' && (b[1] == b'\\' || b[1] == b'/'));
        if !pane.dead && absolute {
            let dirs = out.entry(agent.kind.as_str().to_owned()).or_default();
            if dirs.len() < DIRS_MAX || dirs.contains_key(cwd) {
                let session = dirs.entry(cwd.to_owned()).or_default();
                if session.is_none() {
                    *session = agent.session_id.clone();
                }
            }
        }
    }
    out
}

/// Looks through the agents' folders on every connected machine, the hub's own first.
pub async fn find_all(not_before: &mut HashMap<String, u64>, now: u64) {
    let mut ids = vec![LOCAL_HOST.to_owned()];
    ids.extend(crate::pages_remote::machines());
    for id in ids {
        if not_before.get(&id).is_some_and(|t| now < *t) {
            continue;
        }
        let Some(rt) = registry().get(&id) else { continue };
        let state = rt.borrow().state.clone();
        if state.status != HostStatus::Online {
            continue;
        }
        let mut added = 0;
        for (by, dirs) in agent_folders(&state) {
            match crate::pages_remote::find(&id, &by, &dirs.keys().cloned().collect::<Vec<_>>()).await {
                Ok(found) => {
                    added += found.len();
                    for page in found {
                        let (Some(slug), Some(path)) = (page.get("slug").and_then(Value::as_str), page.get("path").and_then(Value::as_str)) else { continue };
                        if let Some(session) = session_for(path, &dirs) {
                            remember_session(&id, slug, &session);
                        }
                    }
                }
                // its inyon is too old (409) or refused (400, 501): leave it for a while. A timeout
                // or a dropped connection is tried again next time
                Err(e) if matches!(e.status, Some(400 | 409 | 501)) => {
                    not_before.insert(id.clone(), now + RETRY_MS);
                    break;
                }
                Err(_) => break,
            }
        }
        if added > 0 && id != LOCAL_HOST {
            crate::pages::pages().remote.forget(&id);
        }
    }
}

/// The session working in the deepest of `dirs` that holds a found page (`find` only looks inside
/// them). Windows paths match in any case.
pub fn session_for(page: &str, dirs: &BTreeMap<String, Option<String>>) -> Option<String> {
    let parts = |p: &str| p.split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase).collect::<Vec<_>>();
    let page = parts(page);
    dirs.iter()
        .filter_map(|(dir, session)| {
            let dir = parts(dir);
            (!dir.is_empty() && page.starts_with(&dir)).then(|| (dir.len(), session.clone()))
        })
        .max_by_key(|(depth, _)| *depth)
        .and_then(|(_, s)| s)
}

fn remember_session(host: &str, slug: &str, session: &str) {
    if !crate::pages::is_session(session) {
        return;
    }
    let mut all: Map<String, Value> = kv::get("page-sessions").unwrap_or_default();
    all.insert(format!("{host}/{slug}"), Value::String(session.to_owned()));
    while all.len() > SESSIONS_MAX {
        let Some(first) = all.keys().next().cloned() else { break };
        all.remove(&first);
    }
    let _ = kv::set("page-sessions", &all);
}

/// The session a page was found under, when the hub found it.
pub fn session_of(host: &str, slug: &str) -> Option<String> {
    kv::get::<Map<String, Value>>("page-sessions")?.get(&format!("{host}/{slug}"))?.as_str().map(str::to_owned)
}

/// Looks every EVERY_MS while an app is connected or a browser wants page notices.
pub fn start() {
    spawn(async {
        let mut not_before = HashMap::new();
        // a minute after the hub starts, then every EVERY_MS
        let mut wait = 60_000;
        loop {
            sleep(wait).await;
            wait = EVERY_MS;
            if !crate::clients::any() && crate::push::registrations().iter().all(|r| !r.pages) {
                continue;
            }
            find_all(&mut not_before, inyon_core::clock::now_ms()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_live_agent_panes_with_a_full_folder_count() {
        let pane = |cwd: &str, agent: Option<&str>, dead: bool| {
            json!({ "id": "%1", "index": 0, "active": true, "command": "x", "cwd": cwd, "title": "", "width": 80, "height": 24, "dead": dead,
                "agent": agent.map(|k| json!({ "kind": k, "pid": 1, "state": "idle" })) })
        };
        let state: HostState = serde_json::from_value(json!({
            "config": { "id": "laptop", "name": "laptop", "kind": "node" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
            "sessions": [{ "id": "$1", "name": "s", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [
                pane("/home/sam/code/shop", Some("claude"), false),
                pane("/home/sam/code/shop", Some("claude"), false),
                pane("C:\\demo\\app", Some("codex"), false),
                pane("/home/sam/shell", None, false),
                pane("/home/sam/old", Some("claude"), true),
                pane("relative/dir", Some("claude"), false),
            ] }] }]
        }))
        .unwrap();
        let found = agent_folders(&state);
        assert_eq!(found.get("claude").map(|d| d.keys().cloned().collect::<Vec<_>>()), Some(vec!["/home/sam/code/shop".to_owned()]));
        assert_eq!(found.get("codex").map(|d| d.keys().cloned().collect::<Vec<_>>()), Some(vec!["C:\\demo\\app".to_owned()]));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_found_page_belongs_to_the_session_working_in_its_folder() {
        let dirs: BTreeMap<String, Option<String>> = [
            ("/home/sam/code/shop".to_owned(), Some("shop-chat".to_owned())),
            ("/home/sam/code/shop/web".to_owned(), Some("web-chat".to_owned())),
            ("/home/sam/notes".to_owned(), None),
        ]
        .into();
        assert_eq!(session_for("/home/sam/code/shop/private/plans/checkout", &dirs).as_deref(), Some("shop-chat"));
        assert_eq!(session_for("/home/sam/code/shop/web/plans/x", &dirs).as_deref(), Some("web-chat"));
        assert_eq!(session_for("/home/sam/notes/plan", &dirs), None);
        assert_eq!(session_for("/home/sam/code/shopping/plan", &dirs), None);
        // Windows folders, in any case
        let win: BTreeMap<String, Option<String>> = [("C:\\Dev\\Shop".to_owned(), Some("w".to_owned()))].into();
        assert_eq!(session_for("c:\\dev\\shop\\private\\plans\\x", &win).as_deref(), Some("w"));
    }
}
