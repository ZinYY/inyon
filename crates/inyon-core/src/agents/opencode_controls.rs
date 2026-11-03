//! Port of agents/opencode-controls.ts: what a prompt sent through the API should run with, and
//! whether the session is busy. The terminal's footer is the truth: whatever it shows (agent,
//! model, variant) is what OpenCode itself would use for the next message.

use indexmap::IndexSet;
use inyon_protocol::AgentModel;
use serde::Serialize;
use serde_json::Value;

use std::cell::RefCell;
use std::collections::HashMap;

use super::opencode_api::{OpenCodeClient, OpenCodeSelection};
use super::opencode_models::model_from_footer;
use crate::claude::transcript::string_of;
use crate::clock::now_ms;
use crate::hosts::host::Host;
use crate::hosts::http_error::HubResult;
use crate::js::json::{value_string, Js};
use crate::platform::rt::{spawn_task, Deferred};

/// From the footer as the screen reader saw it (ScreenState's model, effort and mode).
pub fn selection_from_footer(model: Option<&str>, effort: Option<&str>, mode: Option<&str>, models: &[AgentModel]) -> Option<OpenCodeSelection> {
    let footer = model.filter(|m| !m.is_empty())?;
    let known = model_from_footer(models, footer)?;
    Some(OpenCodeSelection { model: Some(known.id.clone()), variant: effort.map(str::to_owned), agent: mode.map(str::to_owned) })
}

/// The agent and model the session last ran with, from the server's `/session/{id}` answer.
pub fn stored_selection(info: Option<&Value>) -> OpenCodeSelection {
    let model = info.at("model");
    let provider = model.at("providerID");
    let id = model.at("id");
    OpenCodeSelection {
        model: (provider.truthy() && id.truthy()).then(|| format!("{}/{}", provider.to_js_string(), id.to_js_string())),
        variant: model.at("variant").str().map(str::to_owned),
        agent: info.at("agent").str().map(str::to_owned),
    }
}

/// The last user message's agent, model and variant, for a session the server knows nothing more
/// about.
pub fn selection_from_messages(messages: &[Value]) -> OpenCodeSelection {
    let mut s = OpenCodeSelection::default();
    for row in messages {
        let row = Some(row);
        let info = row.at("info").present().or(row);
        let provider = info.at("model").at("providerID").or(info.at("providerID"));
        let model = info.at("model").at("modelID").or(info.at("modelID"));
        if provider.truthy() && model.truthy() {
            s.model = Some(format!("{}/{}", provider.to_js_string(), model.to_js_string()));
        }
        if info.at("agent").truthy() {
            s.agent = info.at("agent").map(value_string);
        }
        // a new user turn can go back to the default (no variant)
        if info.at("role").str() == Some("user") {
            s.variant = string_of(info.at("model").at("variant").or(info.at("variant")));
        } else if info.at("variant").truthy() {
            s.variant = info.at("variant").map(value_string);
        }
    }
    s
}

/// Whether the session is busy, what runs under it and what permission it waits for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveValue {
    pub working: bool,
    pub retrying: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<Value>,
    /// "2 agents", or null when none run
    pub background: Option<String>,
}

/// `children` are the session's subagent sessions: their work shows as background, their
/// questions as waiting.
pub fn live_value(states: &Value, permissions: &Value, session: &str, children: &IndexSet<String>) -> LiveValue {
    let mine = |id: Option<&Value>| id.str().is_some_and(|id| id == session || children.contains(id));
    let pending = permissions.as_array().and_then(|a| a.iter().find(|p| mine(Some(*p).at("sessionID")))).cloned();
    let kind = |id: &str| Some(states).at(id).at("type").str();
    let busy = |t: Option<&str>| matches!(t, Some("busy" | "retry"));
    let state = kind(session);
    let agents = children.iter().filter(|id| busy(kind(id))).count();
    LiveValue {
        working: busy(state),
        retrying: state == Some("retry"),
        permission: pending,
        background: (agents > 0).then(|| format!("{agents} {}", if agents == 1 { "agent" } else { "agents" })),
    }
}

/// The session's subagent sessions, from the server's `/session` list.
pub fn live_children(sessions: &Value, session: &str) -> IndexSet<String> {
    sessions
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|s| s.get("parentID").and_then(Value::as_str) == Some(session))
        .filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// `storedSelection(client)`: asks the server; nothing for an answer it couldn't give.
pub async fn stored_selection_of(client: &OpenCodeClient) -> OpenCodeSelection {
    let info = client.info().await.ok().flatten();
    stored_selection(info.as_ref())
}

type Live = Deferred<Result<LiveValue, String>>;

thread_local! {
    static LIVE: RefCell<HashMap<String, (u64, Live)>> = RefCell::new(HashMap::new());
}

/// `opencodeLive(host, uri, directory)`: whether the session is busy, what runs under it and what
/// permission it waits for, from its server; one ask per second per session.
pub async fn opencode_live(host: &Host, uri: &str, directory: Option<&str>) -> HubResult<LiveValue> {
    let key = crate::js::stringify(&serde_json::json!([host.id(), uri, directory]));
    let hit = LIVE.with(|l| l.borrow().get(&key).filter(|(at, _)| now_ms().saturating_sub(*at) < 1000).map(|(_, p)| p.clone()));
    let pending = match hit {
        Some(p) => p,
        None => {
            let client = OpenCodeClient::new(host, uri, directory)?;
            let p = spawn_task(async move {
                let sessions = async {
                    if client.session.is_empty() {
                        return Value::Array(vec![]);
                    }
                    client.request("/session", "GET", None, Some(3)).await.ok().flatten().unwrap_or_else(|| Value::Array(vec![]))
                };
                let (states, permissions, sessions) =
                    futures_util::future::join3(client.request("/session/status", "GET", None, Some(3)), client.request("/permission", "GET", None, Some(3)), sessions).await;
                let states = states.map_err(|e| e.message)?.unwrap_or(Value::Null);
                let permissions = permissions.map_err(|e| e.message)?.unwrap_or(Value::Null);
                Ok(live_value(&states, &permissions, &client.session, &live_children(&sessions, &client.session)))
            });
            LIVE.with(|l| {
                let mut l = l.borrow_mut();
                l.insert(key, (now_ms(), p.clone()));
                if l.len() > 100 {
                    l.retain(|_, (at, _)| now_ms().saturating_sub(*at) <= 60_000);
                }
            });
            p
        }
    };
    pending.wait().await.map_err(crate::hosts::http_error::HubError::plain)
}
