//! Port of agents/opencode-api.ts: model ids, the catalog and agents a session's own server
//! lists, and the client that asks it (curl on the session's machine, so the server never has to
//! listen beyond loopback).
//!
//! The CLI owns execution and auth; this adapter speaks its documented local server API
//! (https://opencode.ai/docs/server/ and packages/sdk/js/src/v2/gen/types.gen.ts).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use inyon_protocol::{AgentCatalog, AgentModel, CatalogSource, ModeOption};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::opencode::parse_endpoint;
use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{HubError, HubResult};
use crate::js::json::{self as json, stringify};
use crate::js::text::encode_uri_component;
use crate::js::{fmt_number, to_number, JsStr};

use super::opencode_models::{note_provider_names, variant_names};
use crate::js::json::{value_string, Js};
use crate::js::text::capitalize;

/// What a prompt runs with, as the TUI's footer shows it: "provider/model", the variant, the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OpenCodeSelection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// A model as the API takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenCodeModel {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

/// "provider/model" (the model part may hold more slashes) → its two halves.
pub fn open_code_model(id: &str) -> Result<OpenCodeModel, String> {
    match id.find('/') {
        Some(slash) if slash >= 1 && slash != id.len() - 1 => Ok(OpenCodeModel { provider_id: id[..slash].to_owned(), model_id: id[slash + 1..].to_owned() }),
        _ => Err("Choose an OpenCode provider/model".into()),
    }
}

/// Never return raw provider objects: they include provider.key/options/headers.
pub fn open_code_catalog_from_providers(payload: &Value, now: u64) -> AgentCatalog {
    let p = Some(payload);
    let connected: HashSet<String> = p.at("connected").arr().map(|a| a.iter().map(value_string).collect()).unwrap_or_default();
    let all = p.at("all").arr().map(Vec::as_slice).unwrap_or_default();
    note_provider_names(all.iter().filter_map(|x| Some((x.get("id")?.as_str()?, x.get("name")?.as_str()?))));
    let mut models = vec![];
    for provider in all {
        let provider = Some(provider);
        let Some(pid) = provider.at("id").str().filter(|id| connected.contains(*id)) else { continue };
        for (key, model) in provider.at("models").obj().into_iter().flatten() {
            let model = Some(model);
            if !model.truthy() || model.at("status").str() == Some("deprecated") {
                continue;
            }
            let id = model.at("id").str().unwrap_or(key);
            let pname = provider.at("name").present().map_or_else(|| pid.to_owned(), value_string);
            let mname = model.at("name").present().map_or_else(|| id.to_owned(), value_string);
            models.push(AgentModel {
                id: format!("{pid}/{id}"),
                label: format!("{pname} · {mname}"),
                note: None,
                context: model.at("limit").at("context").num().map(inyon_protocol::Num),
                efforts: Some(variant_names(model.at("variants"))),
                default_effort: None,
                variants: None,
            });
        }
    }
    AgentCatalog { models, source: CatalogSource::Cli, updated_at: now, error: None, modes: None }
}

/// The agents a session can switch to: the primary ones, custom ones included; never the hidden
/// internal ones.
pub fn open_code_agents_from(payload: &Value) -> Vec<ModeOption> {
    payload
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|a| {
            let a = Some(*a);
            a.at("name").str().is_some() && !a.at("hidden").truthy() && a.at("mode").str() != Some("subagent")
        })
        .map(|a| {
            let name = a["name"].as_str().unwrap_or("");
            ModeOption { id: name.to_owned(), label: capitalize(name) }
        })
        .collect()
}

// ---- the client: requests to a session's own server, made with curl on its machine ----

/// The path of a loopback API url, or None for any other url (`new URL(url)` and its checks).
fn loopback_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let port = authority.strip_prefix("127.0.0.1:")?;
    // an empty port, or http's own 80, is no port to `URL`
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.trim_start_matches('0') == "80" {
        return None;
    }
    let path = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    Some(if path.is_empty() { "/".into() } else { path.to_owned() })
}

/// `opencodeRequest(host, url, method, body, timeoutSeconds)`: the parsed JSON answer, None for an
/// empty one. Errors never repeat the server's answer: auth and config endpoints can hold credentials.
pub async fn opencode_request(host: &Host, url: &str, method: &str, body: Option<&Value>, timeout_seconds: u64) -> HubResult<Option<Value>> {
    let path = loopback_path(url).ok_or_else(|| HubError::plain("OpenCode API must use its local loopback server"))?;
    let cmd = format!(
        "curl -sS --max-time {timeout_seconds} -X {} -H 'content-type: application/json' {}--write-out '\\n%{{http_code}}' {}",
        shq(method),
        if body.is_none() { "" } else { "--data-binary @- " },
        shq(url)
    );
    let opts = ExecOptions { input: body.map(|b| stringify(b).into_bytes()), timeout_ms: Some((timeout_seconds + 2) * 1000) };
    let r = host.exec(&cmd, opts).await?;
    let (text, status) = match r.stdout.rfind('\n') {
        Some(i) => (&r.stdout[..i], to_number(&r.stdout[i + 1..])),
        // `slice(0, -1)` and the whole output, as lastIndexOf's -1 makes them
        None => (r.stdout.to16(-1), to_number(&r.stdout)),
    };
    let ok = status.is_finite() && status != 0.0 && (200.0..300.0).contains(&status);
    if r.code != 0 || !ok {
        let status = if r.code != 0 || status.is_nan() { 0.0 } else { status };
        return Err(HubError::plain(if status != 0.0 { format!("OpenCode {path} returned HTTP {}", fmt_number(status)) } else { format!("Could not reach OpenCode {path}") }));
    }
    if text.jtrim().is_empty() {
        return Ok(None);
    }
    json::parse(text).map(Some).ok_or_else(|| HubError::plain(format!("OpenCode {path} returned invalid JSON")))
}

/// A file attached to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptFile {
    #[serde(rename = "type")]
    pub kind: String,
    pub mime: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// One rule for `set_permissions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenCodePermission {
    pub permission: String,
    pub pattern: String,
    pub action: String,
}

/// The API of one OpenCode session's server (`OpenCodeClient`).
#[derive(Clone)]
pub struct OpenCodeClient {
    pub host: Host,
    pub base: String,
    pub session: String,
    pub directory: Option<String>,
}

type Answer = HubResult<Option<Value>>;

fn to_value(v: impl Serialize) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

impl OpenCodeClient {
    pub fn new(host: &Host, uri: &str, directory: Option<&str>) -> HubResult<OpenCodeClient> {
        let ep = parse_endpoint(uri).ok_or_else(|| HubError::plain("Invalid OpenCode endpoint"))?;
        Ok(OpenCodeClient { host: host.clone(), base: ep.base, session: ep.session, directory: directory.map(str::to_owned) })
    }

    pub async fn request(&self, path: &str, method: &str, body: Option<Value>, timeout_seconds: Option<u64>) -> Answer {
        let query = self.directory.as_deref().filter(|d| !d.is_empty()).map(|d| format!("?directory={}", encode_uri_component(d))).unwrap_or_default();
        opencode_request(&self.host, &format!("{}{path}{query}", self.base), method, body.as_ref(), timeout_seconds.unwrap_or(12)).await
    }

    fn session_path(&self, suffix: &str) -> HubResult<String> {
        if self.session.is_empty() {
            return Err(HubError::plain("OpenCode has no selected session"));
        }
        Ok(format!("/session/{}{suffix}", encode_uri_component(&self.session)))
    }

    async fn on_session(&self, suffix: &str, method: &str, body: Option<Value>, timeout: Option<u64>) -> Answer {
        let path = self.session_path(suffix)?;
        self.request(&path, method, body, timeout).await
    }

    pub async fn prompt(&self, text: &str, selection: &OpenCodeSelection, files: &[PromptFile]) -> Answer {
        let mut body = Map::new();
        if let Some(v) = &selection.variant {
            body.insert("variant".into(), Value::String(v.clone()));
        }
        if let Some(a) = &selection.agent {
            body.insert("agent".into(), Value::String(a.clone()));
        }
        if let Some(m) = selection.model.as_deref().filter(|m| !m.is_empty()) {
            body.insert("model".into(), to_value(open_code_model(m).map_err(HubError::plain)?));
        }
        let mut parts = vec![json!({ "type": "text", "text": text })];
        parts.extend(files.iter().map(to_value));
        body.insert("parts".into(), Value::Array(parts));
        self.on_session("/prompt_async", "POST", Some(Value::Object(body)), None).await
    }

    /// Shell and command answers come once the work finishes; never retry a timeout blindly.
    pub async fn shell(&self, command: &str, agent: &str, model: Option<&str>) -> Answer {
        if command.jtrim().is_empty() || agent.is_empty() {
            return Err(HubError::plain("A shell command and OpenCode agent are required"));
        }
        let mut body = Map::new();
        body.insert("command".into(), Value::String(command.into()));
        body.insert("agent".into(), Value::String(agent.into()));
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            body.insert("model".into(), to_value(open_code_model(m).map_err(HubError::plain)?));
        }
        self.on_session("/shell", "POST", Some(Value::Object(body)), Some(300)).await
    }

    pub async fn command(&self, command: &str, args: &str, selection: &OpenCodeSelection) -> Answer {
        let mut body = match to_value(selection) {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        body.insert("command".into(), Value::String(command.strip_prefix('/').unwrap_or(command).into()));
        body.insert("arguments".into(), Value::String(args.into()));
        self.on_session("/command", "POST", Some(Value::Object(body)), Some(300)).await
    }

    pub async fn abort(&self) -> Answer {
        self.on_session("/abort", "POST", None, None).await
    }
    pub async fn status(&self) -> Answer {
        self.request("/session/status", "GET", None, None).await
    }
    /// the conversation itself: title, agent and model last used, tokens, cost, share link, what /undo reverted
    pub async fn info(&self) -> Answer {
        self.on_session("", "GET", None, None).await
    }
    pub async fn messages(&self) -> Answer {
        self.on_session("/message", "GET", None, None).await
    }
    /// /undo: back to before `message_id` (its files too); /redo puts it back
    pub async fn revert(&self, message_id: &str) -> Answer {
        self.on_session("/revert", "POST", Some(json!({ "messageID": message_id })), Some(60)).await
    }
    pub async fn unrevert(&self) -> Answer {
        self.on_session("/unrevert", "POST", None, Some(60)).await
    }
    pub async fn share(&self) -> Answer {
        self.on_session("/share", "POST", None, Some(60)).await
    }
    pub async fn unshare(&self) -> Answer {
        self.on_session("/share", "DELETE", None, Some(60)).await
    }
    pub async fn summarize(&self, model: &str) -> Answer {
        let m = to_value(open_code_model(model).map_err(HubError::plain)?);
        self.on_session("/summarize", "POST", Some(m), Some(300)).await
    }
    pub async fn permissions(&self) -> Answer {
        self.request("/permission", "GET", None, None).await
    }
    pub async fn reply(&self, id: &str, reply: &str) -> Answer {
        self.request(&format!("/permission/{}/reply", encode_uri_component(id)), "POST", Some(json!({ "reply": reply })), None).await
    }
    pub async fn set_permissions(&self, permission: &[OpenCodePermission]) -> Answer {
        self.on_session("", "PATCH", Some(json!({ "permission": to_value(permission) })), None).await
    }
    /// questions the agent asked (its question tool), one request per tool call
    pub async fn questions(&self) -> Answer {
        self.request("/question", "GET", None, None).await
    }
    /// one array of chosen labels (or a typed answer) per question, in order
    pub async fn reply_question(&self, id: &str, answers: &[Vec<String>]) -> Answer {
        self.request(&format!("/question/{}/reply", encode_uri_component(id)), "POST", Some(json!({ "answers": answers })), None).await
    }
    pub async fn reject_question(&self, id: &str) -> Answer {
        self.request(&format!("/question/{}/reject", encode_uri_component(id)), "POST", None, None).await
    }
    /// shows a session in the terminal, after one was created here
    pub async fn select_session(&self, session_id: &str) -> Answer {
        self.request("/tui/select-session", "POST", Some(json!({ "sessionID": session_id })), None).await
    }
    /// drops the server's cached state for the folder (providers, credentials): read again on the next request
    pub async fn dispose(&self) -> Answer {
        self.request("/instance/dispose", "POST", None, None).await
    }
}

thread_local! {
    static RECENT: RefCell<HashMap<String, (u64, AgentCatalog)>> = RefCell::new(HashMap::new());
}

/// The models of every connected provider, and the agents, from a session's own server.
pub async fn opencode_catalog(host: &Host, uri: &str, directory: Option<&str>) -> HubResult<AgentCatalog> {
    let client = OpenCodeClient::new(host, uri, directory)?;
    let (providers, agents) = futures_util::future::join(client.request("/provider", "GET", None, None), client.request("/agent", "GET", None, None)).await;
    let providers = providers?.unwrap_or(Value::Null);
    let agents = agents.ok().flatten().unwrap_or_else(|| Value::Array(vec![]));
    let modes = open_code_agents_from(&agents);
    let mut catalog = open_code_catalog_from_providers(&providers, now_ms());
    if !modes.is_empty() {
        catalog.modes = Some(modes);
    }
    RECENT.with(|r| r.borrow_mut().insert(client.base.clone(), (now_ms(), catalog.clone())));
    Ok(catalog)
}

/// The same, kept for a minute: the footer is matched against it on every screen read.
pub async fn opencode_catalog_cached(host: &Host, uri: &str, directory: Option<&str>) -> HubResult<AgentCatalog> {
    if let Some(ep) = parse_endpoint(uri) {
        if let Some((at, c)) = RECENT.with(|r| r.borrow().get(&ep.base).cloned()) {
            if now_ms().saturating_sub(at) < 60_000 {
                return Ok(c);
            }
        }
    }
    opencode_catalog(host, uri, directory).await
}

/// After a provider was connected or disconnected on the machine: ask again next time.
pub fn forget_open_code_catalogs() {
    RECENT.with(|r| r.borrow_mut().clear());
}
