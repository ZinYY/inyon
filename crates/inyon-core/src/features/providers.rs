//! Port of features/providers.ts: OpenCode's model providers on a machine: which are connected,
//! connecting one more, and disconnecting. Connecting runs OpenCode's own `opencode auth login` on
//! the machine (features/signin.rs drives it from the app), so a key typed in the app goes to that
//! command and nowhere else; the hub never reads a credential back. Disconnecting is
//! `opencode auth logout`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use inyon_protocol::{AgentKind, AgentState, OpenCodeProvider, ProviderVia, ProvidersReport};
use serde::Serialize;
use serde_json::Value;

use super::signin::{start_signin, SignInStart};
use crate::agents::opencode::{parse_endpoint, via_open_code_api};
use crate::agents::opencode_api::{forget_open_code_catalogs, opencode_request, OpenCodeClient};
use crate::agents::opencode_db::OC_PATH;
use crate::agents::opencode_models::{forget_open_code_models, note_provider_names, provider_name};
use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{HubError, HubResult};
use crate::hosts::registry::registry;
use crate::js::text::{encode_uri_component, locale_compare, lower, JsStr};
use crate::platform::rt::{spawn, spawn_task, Deferred};
use crate::re;

/// the order OpenCode's own picker opens with, before the rest alphabetically
const POPULAR: [&str; 7] = ["opencode", "openai", "github-copilot", "google", "anthropic", "openrouter", "vercel"];

fn valid_id(id: &str) -> bool {
    re!(r"^[\w.-]{1,64}$").is_match(id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogueProvider {
    pub id: String,
    pub name: String,
    pub env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Catalogue {
    pub providers: Vec<CatalogueProvider>,
    pub connected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthCredential {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthEnv {
    pub name: String,
    pub variable: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AuthList {
    pub credentials: Vec<AuthCredential>,
    pub env: Vec<AuthEnv>,
}

/// `opencode auth list`: saved credentials ("●  DeepSeek api"), then providers reached through
/// environment variables ("●  Groq GROQ_API_KEY"). Names are OpenCode's display names.
pub fn parse_auth_list(text: &str) -> AuthList {
    #[derive(PartialEq)]
    enum Section {
        Credentials,
        Env,
    }
    let mut out = AuthList::default();
    let mut section: Option<Section> = None;
    for raw in re!(r"\x1b\[[0-9;]*m").replace_all(text, "").split('\n') {
        let line = raw.jtrim();
        if re!(r"^┌\s+Credentials\b").is_match(line) {
            section = Some(Section::Credentials);
        } else if re!(r"^┌\s+Environment\b").is_match(line) {
            section = Some(Section::Env);
        } else if line.starts_with('└') {
            section = None;
        }
        let Some(s) = &section else { continue };
        let Some(m) = re!(r"^[●◇◆■]\s+(.+?)\s+(\S+)$").captures(line) else { continue };
        if *s == Section::Credentials {
            out.credentials.push(AuthCredential { name: m[1].to_owned(), kind: m[2].to_owned() });
        } else {
            out.env.push(AuthEnv { name: m[1].to_owned(), variable: m[2].to_owned() });
        }
    }
    out
}

/// Only ids, names, environment variables and the connected list are kept: a provider entry also carries its key.
pub fn catalogue_from(payload: &Value) -> Catalogue {
    let strings = |v: Option<&Value>| -> Vec<String> { v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default() };
    let providers: Vec<CatalogueProvider> = payload
        .get("all")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|p| {
                    let id = p.get("id").and_then(Value::as_str).filter(|id| valid_id(id))?;
                    let name = p.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map_or_else(|| provider_name(id), str::to_owned);
                    Some(CatalogueProvider { id: id.to_owned(), name, env: strings(p.get("env")) })
                })
                .collect()
        })
        .unwrap_or_default();
    note_provider_names(providers.iter().map(|p| (p.id.as_str(), p.name.as_str())));
    Catalogue { providers, connected: strings(payload.get("connected")) }
}

/// Starts OpenCode's server for a moment in an empty folder, reads its provider list and stops
/// it. About four seconds, so the answer is kept for ten minutes; a session's own server is
/// used instead when the machine has one.
pub fn catalogue_sh() -> String {
    format!(
        r#"{OC_PATH}
d=$(mktemp -d) || exit 1
cd "$d" || exit 1
opencode serve --hostname 127.0.0.1 --port 0 >"$d/log" 2>&1 &
pid=$!
port=
for i in $(seq 1 120); do
  sleep 0.25
  port=$(grep -o 'http://127.0.0.1:[0-9]*' "$d/log" 2>/dev/null | head -1 | sed 's/.*://')
  [ -n "$port" ] && curl -s -m 1 "http://127.0.0.1:$port/global/health" 2>/dev/null | grep -q healthy && break
  port=
done
[ -n "$port" ] && curl -s -m 30 "http://127.0.0.1:$port/provider"
kill "$pid" 2>/dev/null; sleep 0.3; kill -9 "$pid" 2>/dev/null
cd / && rm -rf "$d""#
    )
}

type Pending = Deferred<HubResult<Catalogue>>;

thread_local! {
    static CATALOGUES: RefCell<HashMap<String, (u64, Pending)>> = RefCell::new(HashMap::new());
}

async fn catalogue(host: &Host, uri: Option<String>, cwd: Option<String>) -> HubResult<Catalogue> {
    let id = host.id();
    let hit = CATALOGUES.with(|c| c.borrow().get(&id).cloned());
    if let Some((at, value)) = hit {
        if now_ms().saturating_sub(at) < 600_000 {
            return value.wait().await;
        }
    }
    let h = host.clone();
    let value = spawn_task(async move {
        if let Some(ep) = uri.as_deref().and_then(parse_endpoint) {
            let dir = cwd.as_deref().filter(|c| !c.is_empty()).map(|c| format!("?directory={}", encode_uri_component(c))).unwrap_or_default();
            if let Ok(Some(live)) = opencode_request(&h, &format!("{}/provider{dir}", ep.base), "GET", None, 12).await {
                return Ok(catalogue_from(&live));
            }
        }
        let r = h.exec(&catalogue_sh(), ExecOptions::timeout(90_000)).await?;
        let Some(payload) = crate::js::json::parse(r.stdout.jtrim()) else {
            return Err(HubError::http(503, "OpenCode's provider list couldn't be read on that machine. Is OpenCode installed there?"));
        };
        Ok(catalogue_from(&payload))
    });
    CATALOGUES.with(|c| c.borrow_mut().insert(id.clone(), (now_ms(), value.clone())));
    let failed = value.clone();
    spawn(async move {
        if failed.wait().await.is_err() {
            CATALOGUES.with(|c| c.borrow_mut().remove(&id));
        }
    });
    value.wait().await
}

async fn auth_list(host: &Host) -> HubResult<AuthList> {
    let r = host.exec(&format!("{OC_PATH}; opencode auth list 2>/dev/null"), ExecOptions::timeout(30_000)).await?;
    Ok(parse_auth_list(&r.stdout))
}

/// A provider's id from the name `auth list` prints, through the catalogue.
fn id_for(cat: &Catalogue, name: &str) -> String {
    if let Some(p) = cat.providers.iter().find(|p| p.name == name) {
        return p.id.clone();
    }
    let slug = re!(r"[^a-z0-9]+", "g").replace_all(&lower(name), "-").into_owned();
    re!(r"^-|-$", "g").replace_all(&slug, "").into_owned()
}

pub fn merge_providers(cat: &Catalogue, auth: &AuthList) -> Vec<OpenCodeProvider> {
    let fresh = |id: &str, name: &str| OpenCodeProvider { id: id.to_owned(), name: name.to_owned(), connected: false, via: None, r#type: None, env: None };
    let mut by_id: IndexMap<String, OpenCodeProvider> = IndexMap::new();
    for p in &cat.providers {
        by_id.insert(p.id.clone(), OpenCodeProvider { env: (!p.env.is_empty()).then(|| p.env.clone()), ..fresh(&p.id, &p.name) });
    }
    for c in &auth.credentials {
        let id = id_for(cat, &c.name);
        let p = by_id.get(&id).cloned().unwrap_or_else(|| fresh(&id, &c.name));
        by_id.insert(id, OpenCodeProvider { connected: true, via: Some(ProviderVia::Credential), r#type: Some(c.kind.clone()), ..p });
    }
    for e in &auth.env {
        let id = id_for(cat, &e.name);
        let p = by_id.get(&id).cloned();
        if p.as_ref().is_some_and(|p| p.connected) {
            continue;
        }
        let p = p.unwrap_or_else(|| fresh(&id, &e.name));
        by_id.insert(id, OpenCodeProvider { connected: true, via: Some(ProviderVia::Env), env: Some(vec![e.variable.clone()]), ..p });
    }
    // connected without a saved credential: OpenCode's own free models, or a key in its config file
    for id in &cat.connected {
        if let Some(p) = by_id.get_mut(id) {
            if !p.connected {
                p.connected = true;
            }
        }
    }
    let popular = |id: &str| POPULAR.iter().position(|p| *p == id);
    let rank = |p: &OpenCodeProvider| {
        if p.connected {
            0
        } else if popular(&p.id).is_some() {
            1
        } else {
            2
        }
    };
    let mut list: Vec<OpenCodeProvider> = by_id.into_values().collect();
    list.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| if rank(a) == 1 { popular(&a.id).cmp(&popular(&b.id)) } else { locale_compare(&a.name, &b.name) }));
    list
}

/// The pane's own OpenCode server, when it runs one.
fn api_pane(host_id: &str, pane_id: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(pane) = pane_id.and_then(|id| registry().find_pane(host_id, id)).map(|f| f.pane) else { return (None, None) };
    let uri = pane.agent.as_ref().filter(|a| a.kind == AgentKind::Opencode && via_open_code_api(a.transcript_path.as_deref())).and_then(|a| a.transcript_path.clone());
    (uri, Some(pane.cwd))
}

pub async fn list_providers(host_id: &str, pane_id: Option<&str>) -> HubResult<ProvidersReport> {
    let host = registry().host(host_id)?;
    let (uri, cwd) = api_pane(host_id, pane_id);
    let (cat, auth) = futures_util::future::join(catalogue(&host, uri, cwd), auth_list(&host)).await;
    Ok(match cat.and_then(|cat| auth.map(|auth| (cat, auth))) {
        Ok((cat, auth)) => ProvidersReport { providers: merge_providers(&cat, &auth), updated_at: now_ms(), error: None },
        Err(e) => ProvidersReport { providers: vec![], updated_at: now_ms(), error: Some(e.message) },
    })
}

/// Runs `opencode auth login -p <provider>` on the machine, out of sight; the sign-in sheet drives it.
pub async fn connect_provider(host_id: &str, provider: &str) -> HubResult<SignInStart> {
    if !valid_id(provider) {
        return Err(HubError::http(400, "Unknown provider"));
    }
    start_signin(host_id, "opencode", Some(provider)).await
}

/// `opencode auth logout <provider>`; a provider connected through the environment can't be, and says so.
pub async fn disconnect_provider(host_id: &str, provider: &str) -> HubResult<()> {
    if !valid_id(provider) {
        return Err(HubError::http(400, "Unknown provider"));
    }
    let host = registry().host(host_id)?;
    let r = host.exec(&format!("{OC_PATH}; opencode auth logout {} 2>&1", shq(provider)), ExecOptions::timeout(30_000)).await?;
    let out = re!(r"\x1b\[[0-9;]*m", "g").replace_all(&r.stdout, "").into_owned();
    if !out.contains("Logout successful") {
        let auth = auth_list(&host).await.ok();
        let none = Catalogue::default();
        let env = auth.as_ref().and_then(|a| a.env.iter().find(|e| lower(&e.name) == lower(provider) || id_for(&none, &e.name) == provider));
        if let Some(env) = env {
            return Err(HubError::http(409, format!("{} is connected through {} in the machine's environment. Unset it there to disconnect.", env.name, env.variable)));
        }
        let last = out.jtrim().split('\n').next_back().unwrap_or("");
        return Err(HubError::http(
            409,
            if out.contains("No credentials found") {
                "That provider has no saved credential to remove"
            } else if last.is_empty() {
                "OpenCode did not log out"
            } else {
                last
            },
        ));
    }
    after_credential_change(host_id).await
}

/// A credential was added or removed on the machine: Inyon's lists are read again, and sessions
/// Inyon started (the ones with their own server) are told to reload theirs. A running plain
/// `opencode` reads credentials when it starts, so it sees the change after a restart.
pub async fn after_credential_change(host_id: &str) -> HubResult<()> {
    CATALOGUES.with(|c| c.borrow_mut().remove(host_id));
    forget_open_code_models(host_id);
    forget_open_code_catalogs();
    if let Some(rt) = registry().get(host_id) {
        let (host, servers) = {
            let r = rt.borrow();
            let mut seen = HashSet::new();
            let mut servers: Vec<(String, String)> = vec![];
            for p in r.state.sessions.iter().flat_map(|s| s.windows.iter().flat_map(|w| w.panes.iter())) {
                let Some(a) = p.agent.as_ref() else { continue };
                if a.kind != AgentKind::Opencode || !via_open_code_api(a.transcript_path.as_deref()) || a.state == AgentState::Working {
                    continue;
                }
                let uri = a.transcript_path.clone().unwrap_or_default();
                let Some(ep) = parse_endpoint(&uri) else { continue };
                if !seen.insert(format!("{} {}", ep.base, p.cwd)) {
                    continue;
                }
                servers.push((uri, p.cwd.clone()));
            }
            (r.host.clone(), servers)
        };
        let disposals = servers.iter().map(|(uri, cwd)| {
            let host = host.clone();
            async move {
                if let Ok(client) = OpenCodeClient::new(&host, uri, Some(cwd)) {
                    let _ = client.dispose().await;
                }
            }
        });
        futures_util::future::join_all(disposals).await;
    }
    registry().refresh(host_id, 200);
    Ok(())
}

#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
