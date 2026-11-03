//! The inyon command's own way into the API, from 0.22: `inyon hub machines`, `inyon sessions`,
//! `inyon pages --hub` and `inyon send`. On a joined machine it uses the token that machine keeps
//! for its link to the hub; on the hub's own machine, the local token the hub wrote as it started.
//! Either may read the machines, their sessions and the pages. A message for an agent is held and
//! typed in once the agent is free (inyon_core::claude::held); from a joined machine only when
//! Settings allows it (`machinesSend`, off unless turned on), since that types into agents on other
//! machines. On the hub's own machine it may also read a session's latest messages and answer
//! the question an agent asks (`inyon read`, `inyon answer`), which the companion agent uses, and
//! start or stop the companion (`inyon companion`).
//! Every other route answers 403.

use axum::http::Method;
use inyon_core::claude::held;
use inyon_core::clock::now_ms;
use inyon_core::config::load_settings;
use inyon_core::hosts::registry::{registry, token_expired};
use inyon_protocol::{HostConfig, HostKind};
use serde_json::{json, Value};

use crate::http::{as_str, field, json as answer_json, json_of, ok_json, pane_param, same, sha256, Req, Resp};

/// Who the request comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Caller {
    /// the inyon command on the hub's machine
    Local,
    /// a joined machine, by its id
    Machine(String),
}

pub const SCOPE_REFUSED: &str = "The inyon command can read machines, sessions and pages here, and hold messages for agents. Do the rest in the app.";
pub const LOCAL_ONLY: &str = "Only the inyon command on the hub's own machine can read conversations, answer questions and run the companion.";
pub const SEND_OFF: &str = "This hub doesn't take messages from its machines. Turn on Settings > Machines can message agents in the app to allow it.";

/// The machine whose link token the request carries, or the local token from the hub's machine.
pub fn caller(req: &Req) -> Option<Caller> {
    let token = crate::phones::bearer(req)?;
    if crate::index::from_this_machine(req) {
        return Some(Caller::Local);
    }
    machine_by_token(&token).map(|c| Caller::Machine(c.id))
}

/// A joined machine by its token, as `/node` finds it; a pairing command that expired unused is no one.
pub fn machine_by_token(token: &str) -> Option<HostConfig> {
    let hash = sha256(token);
    registry()
        .configs()
        .into_iter()
        .find(|c| c.kind == HostKind::Node && c.token_hash.as_deref().is_some_and(|h| !h.is_empty() && same(h, &hash)))
        .filter(|c| !token_expired(c, now_ms()))
}

/// A held message for a pane, by its route: `/api/hosts/:id/panes/:pane/held`.
fn held_route(path: &str) -> bool {
    inyon_core::re!(r"^/api/hosts/[^/]+/panes/[^/]+/held$").is_match(path)
}

/// A pane route by its last part: `/api/hosts/:id/panes/:pane/<last>`.
fn pane_route(path: &str, last: &str) -> bool {
    path.strip_suffix(last).and_then(|p| p.strip_suffix('/')).is_some_and(|p| inyon_core::re!(r"^/api/hosts/[^/]+/panes/[^/]+$").is_match(p))
}

/// The hub's answer to the inyon command, or a 403 for a route it may not use.
pub async fn as_caller(mut req: Req, who: Caller) -> Resp {
    let get = req.method == Method::GET || req.method == Method::HEAD;
    let path = req.path.clone();
    let res = match path.as_str() {
        "/api/overview" if get => Ok(overview()),
        "/api/pages" if get => crate::pages::list(req).await,
        p if req.method == Method::POST && held_route(p) => {
            if matches!(who, Caller::Machine(_)) && !sends_allowed() {
                return answer_json(403, &json!({ "error": SEND_OFF, "reason": "send-off" }));
            }
            hold(&mut req).await
        }
        "/api/companion" => {
            use inyon_core::features::companion;
            if who != Caller::Local {
                return answer_json(403, &json!({ "error": LOCAL_ONLY, "reason": "local-only" }));
            }
            match req.method {
                Method::GET => Ok(json_of(&companion::status())),
                Method::POST => match req.body().await {
                    Ok(b) => companion::start(b.get("agent").and_then(Value::as_str).unwrap_or("claude")).await.map(|v| json_of(&v)),
                    Err(e) => Err(e),
                },
                Method::DELETE => companion::stop().await.map(|()| json_of(&json!({ "ok": true }))),
                _ => return answer_json(405, &json!({ "error": "Method not allowed" })),
            }
        }
        p if (get && pane_route(p, "history")) || (req.method == Method::POST && pane_route(p, "answer")) => {
            if who != Caller::Local {
                return answer_json(403, &json!({ "error": LOCAL_ONLY, "reason": "local-only" }));
            }
            let (id, pane) = (req.param("id"), pane_param(&req.param("pane")));
            if get {
                inyon_core::features::companion::history(&id, &pane, crate::core_routes::history_limit(&req.q("limit"))).await.map(|v| json_of(&v))
            } else {
                match req.body().await {
                    Ok(b) => {
                        let choice = b.get("choice").and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))).unwrap_or_default();
                        inyon_core::features::companion::answer_by_choice(&id, &pane, &choice).await.map(|v| json_of(&v))
                    }
                    Err(e) => Err(e),
                }
            }
        }
        _ => return answer_json(403, &json!({ "error": SCOPE_REFUSED, "reason": "scope" })),
    };
    crate::http::answer(res)
}

/// Settings > Machines can message agents.
pub fn sends_allowed() -> bool {
    load_settings().0.get("machinesSend").and_then(Value::as_bool) == Some(true)
}

/// The machines and their sessions, as a companion sees them plus each pane's command and folder.
pub fn overview() -> Resp {
    let mut hosts = serde_json::to_value(registry().overview()).unwrap_or_default();
    crate::companions::shape_hosts(&mut hosts, &MACHINE_PANE);
    ok_json(&json!({ "hosts": hosts }))
}

pub const MACHINE_PANE: [&str; 10] = ["id", "index", "active", "title", "label", "dead", "agent", "companion", "command", "cwd"];

async fn hold(req: &mut Req) -> inyon_core::hosts::http_error::HubResult<Resp> {
    let b = req.body().await?;
    let text = as_str(field(&b, "text")?).unwrap_or_default();
    let (id, pane) = (req.param("id"), pane_param(&req.param("pane")));
    Ok(json_of(&held::hold(&id, &pane, &text)?))
}

#[cfg(test)]
#[path = "machines_tests.rs"]
mod tests;
