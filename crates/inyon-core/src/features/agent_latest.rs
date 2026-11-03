//! Port of features/agent-latest.ts: the newest published version of each agent, from its npm
//! package's `latest` tag. Asked at most once an hour per package, with a short timeout, and a
//! failed lookup is remembered for a few minutes so a machine's Setup tab never waits on a
//! registry that's down. Nothing about the user or the machine goes out: the request is the
//! package name alone.

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;
use inyon_protocol::{AgentKind, AGENT_KINDS};
use serde_json::Value;

use crate::clock::now_ms;
use crate::js::text::JsStr;
use crate::platform::fetch::{fetch, FetchRequest};
use crate::platform::rt::{spawn_task, Deferred};
use crate::re;

const REGISTRY: &str = "https://registry.npmjs.org";
const TTL: u64 = 60 * 60_000;
const FAIL_TTL: u64 = 5 * 60_000;
const TIMEOUT: u64 = 4000;

#[derive(Clone)]
struct Entry {
    version: Option<String>,
    at: u64,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
    static INFLIGHT: RefCell<HashMap<String, Deferred<Option<String>>>> = RefCell::new(HashMap::new());
}

/// The digits of a version: "v2.1.296 (Claude Code)" and "codex-cli 0.49.0" both parse.
pub fn parse_version(s: Option<&str>) -> Option<Vec<f64>> {
    let s = s.unwrap_or("");
    let s = re!(r"^\s*v", "i").replace(s, "");
    let m = re!(r"\d+(?:\.\d+)+").find(&s)?;
    Some(m.as_str().split('.').map(crate::js::to_number).collect())
}

/// -1 when a is older than b, 1 when newer, 0 when equal or when either can't be read.
pub fn compare_versions(a: Option<&str>, b: Option<&str>) -> i32 {
    let (Some(x), Some(y)) = (parse_version(a), parse_version(b)) else { return 0 };
    for i in 0..x.len().max(y.len()) {
        let d = x.get(i).copied().unwrap_or(0.0) - y.get(i).copied().unwrap_or(0.0);
        if d != 0.0 {
            return if d < 0.0 { -1 } else { 1 };
        }
    }
    0
}

/// The package's latest version, or None when the registry can't say right now.
pub async fn latest_npm_version(pkg: &str) -> Option<String> {
    let hit = CACHE.with(|c| c.borrow().get(pkg).cloned());
    if let Some(hit) = hit {
        if now_ms().saturating_sub(hit.at) < if hit.version.is_some() { TTL } else { FAIL_TTL } {
            return hit.version;
        }
    }
    if let Some(pending) = INFLIGHT.with(|i| i.borrow().get(pkg).cloned()) {
        return pending.wait().await;
    }
    let key = pkg.to_owned();
    let url = format!("{REGISTRY}/{pkg}/latest");
    let p = spawn_task(async move {
        let mut version = None;
        // unreachable, slow or not JSON: the agent simply has no "latest" until the next try
        if let Ok(res) = fetch(FetchRequest::get(url, TIMEOUT).header("accept", "application/json")).await {
            if res.ok() {
                if let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(&res.body) {
                    if let Some(Value::String(v)) = body.get("version") {
                        if parse_version(Some(v)).is_some() {
                            version = Some(v.jtrim().to_owned());
                        }
                    }
                }
            }
        }
        CACHE.with(|c| c.borrow_mut().insert(key.clone(), Entry { version: version.clone(), at: now_ms() }));
        INFLIGHT.with(|i| i.borrow_mut().remove(&key));
        version
    });
    // the task may have finished already (a fetch that failed at once): then it's in the cache
    if !p.is_settled() {
        INFLIGHT.with(|i| i.borrow_mut().insert(pkg.to_owned(), p.clone()));
    }
    p.wait().await
}

/// The latest version of every agent with a package, looked up in parallel. Never fails.
pub async fn latest_agent_versions() -> IndexMap<AgentKind, String> {
    let lookups = AGENT_KINDS.iter().filter_map(|k| k.def().latest.as_ref().map(|l| (*k, l.npm.clone()))).map(|(k, pkg)| async move { (k, latest_npm_version(&pkg).await) });
    let found = futures_util::future::join_all(lookups).await;
    found.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()
}

/// For tests.
pub fn forget_latest() {
    CACHE.with(|c| c.borrow_mut().clear());
    INFLIGHT.with(|i| i.borrow_mut().clear());
}

#[cfg(test)]
#[path = "agent_latest_tests.rs"]
mod tests;
