//! Port of companions.ts: companion apps (Legato and the like), contract version 1. A companion
//! pairs with a short code the signed-in web app (or `inyon hub companion-code`) shows, and gets a
//! bearer token with four scopes: read the overview, watch chats, answer prompts and send text.
//! Every request it makes carries `X-Inyon-Contract`, and the hub checks its scope on every route
//! and WebSocket frame; the rest of the API answers 403. The token is kept as its hash only, as a
//! phone's is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use axum::http::Method;
use futures_util::future::LocalBoxFuture;
use inyon_core::claude::actions;
use inyon_core::claude::send_receipts::send_once;
use inyon_core::claude::watcher::watchers;
use inyon_core::clock::now_ms;
use inyon_core::hosts::http_error::HubResult;
use inyon_core::hosts::registry::registry;
use inyon_core::js::JsStr;
use inyon_core::platform::store::kv;
use inyon_protocol::{prompt_key, CompanionScope, HostStatus, SendStatus, ServerMsg};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::auth::Closer;
use crate::http::{json as answer_json, ok_json, pane_param, random_base64url, random_bytes, random_hex, same, sha256, str_or_empty, Req, Resp};

pub const CONTRACTS: [u32; 1] = [1];
pub const CONTRACT: u32 = 1;
pub const SCOPES: [CompanionScope; 4] = [CompanionScope::Overview, CompanionScope::Chat, CompanionScope::Answer, CompanionScope::Send];
/// how long a code stays claimable
pub const COMPANION_CODE_TTL: u64 = 5 * 60_000;
/// failed claims one address gets a minute, and all addresses together
const CLAIMS_PER_MINUTE: u32 = 8;
const ALL_CLAIMS_PER_MINUTE: u32 = 30;
/// how often a companion's lastSeen is written
const SEEN_EVERY: u64 = 60_000;
/// no 0, O, 1, I or L: a code is read off one screen and typed into another
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionRecord {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// sha256 of the token, hex
    pub token_hash: String,
    pub scopes: Vec<CompanionScope>,
    pub contract_version: u32,
    pub paired_at: u64,
    pub last_seen: u64,
}

#[derive(Default)]
struct State {
    companions: Vec<CompanionRecord>,
    /// one code at a time: a new one ends the last
    current: Option<(String, u64)>,
    failures: HashMap<String, (u32, u64)>,
    all_failures: (u32, u64),
    sockets: Vec<(u64, Closer, String)>,
    next: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Reads the paired companions from the records (the module's top level in the TS).
pub fn init() {
    let all: Vec<CompanionRecord> = kv::get("companions").unwrap_or_default();
    with(|s| s.companions = all);
}

fn save() {
    let all = with(|s| s.companions.clone());
    if let Err(e) = kv::set("companions", &all) {
        inyon_core::platform::console::error(&format!("couldn't save the companion apps: {e}"));
    }
}

/// A refusal a companion can act on: the message to show, and a reason that stays the same.
pub fn refuse(status: u16, reason: &str, error: &str) -> Resp {
    answer_json(status, &json!({ "error": error, "reason": reason }))
}

fn scope_name(s: CompanionScope) -> &'static str {
    match s {
        CompanionScope::Overview => "overview",
        CompanionScope::Chat => "chat",
        CompanionScope::Answer => "answer",
        CompanionScope::Send => "send",
    }
}

// ---------- the code ----------

fn new_code() -> String {
    let mut out = String::new();
    while out.len() < 6 {
        // 248 is the largest multiple of 31 under 256, so every character is as likely
        for b in random_bytes(8) {
            if b < 248 && out.len() < 6 {
                out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
            }
        }
    }
    out
}

/// A fresh code for the web app or the inyon command to show.
/// Contract 1.1: the code and the claim carry the SHA-256 of the certificate the hub serves itself,
/// for a companion to pin. Left out on plain http and behind a proxy.
pub fn with_tls(mut v: Value, fingerprint: Option<String>) -> Value {
    if let Some(fp) = fingerprint {
        v["tlsFingerprint"] = json!(fp);
    }
    v
}

pub fn new_companion_code(now: u64) -> Value {
    let code = new_code();
    let expires_at = now + COMPANION_CODE_TTL;
    with(|s| s.current = Some((code.clone(), expires_at)));
    with_tls(json!({ "code": code, "expiresAt": expires_at }), crate::tls::served_sha256())
}

fn count_failure(s: &mut State, ip: &str, now: u64) {
    s.failures.retain(|_, (_, reset)| *reset >= now);
    let a = s.failures.entry(ip.to_owned()).or_insert((0, now + 60_000));
    a.0 += 1;
    if s.all_failures.1 < now {
        s.all_failures = (0, now + 60_000);
    }
    s.all_failures.0 += 1;
}

/// The code typed as it may come: any case, with spaces or a dash in it.
fn clean_code(v: Option<&Value>) -> String {
    str_or_empty(v).chars().filter(|c| !c.is_whitespace() && *c != '\u{feff}' && *c != '-').collect::<String>().to_uppercase()
}

/// A companion hands in the code. Wrong and expired codes read the same; failures are counted per
/// address and in all.
pub fn claim_companion(ip: &str, b: &Value, now: u64) -> Resp {
    let (mine, all) = with(|s| (s.failures.get(ip).is_some_and(|(n, reset)| *reset >= now && *n >= CLAIMS_PER_MINUTE), if s.all_failures.1 >= now { s.all_failures.0 } else { 0 }));
    if mine || !crate::limits::shared_cap_allows("companion code", ip, all, ALL_CLAIMS_PER_MINUTE, now) {
        return refuse(429, "rate-limit", "Too many attempts. Try again in a minute.");
    }
    let code = clean_code(b.get("code"));
    let good = with(|s| {
        let good = s.current.as_ref().is_some_and(|(c, exp)| *exp > now && same(&code, c));
        if good {
            s.current = None;
        } else {
            count_failure(s, ip, now);
        }
        good
    });
    if !good {
        return refuse(401, "bad-code", "That code is not right or has expired. Make a new one in Inyon.");
    }
    let token = random_base64url(32);
    let raw_name = str_or_empty(b.get("name"));
    let name = raw_name.jtrim().slice16(0, Some(60)).to_owned();
    let name = if name.is_empty() { "Companion app".to_owned() } else { name };
    let app = b.get("app").and_then(Value::as_str).map(|a| a.jtrim().slice16(0, Some(60)).to_owned()).filter(|a| !a.is_empty());
    with(|s| {
        s.companions.push(CompanionRecord {
            id: random_hex(6),
            name,
            app,
            token_hash: sha256(&token),
            scopes: SCOPES.to_vec(),
            contract_version: CONTRACT,
            paired_at: now,
            last_seen: now,
        })
    });
    save();
    // a companion that pins the certificate gets it here too (contract 1.1)
    ok_json(&with_tls(json!({ "token": token, "hubId": crate::phones::hub_id(), "scopes": SCOPES, "contractVersion": CONTRACT }), crate::tls::served_sha256()))
}

// ---------- a companion's requests ----------

/// `req.headers.get('authorization')?.match(/^Bearer\s+(\S+)$/i)?.[1]`, as phones read it.
fn bearer(req: &Req) -> Option<String> {
    let h = req.header("authorization")?;
    let rest = h.get(..6).filter(|p| p.eq_ignore_ascii_case("bearer")).map(|_| &h[6..])?;
    let token = rest.trim_start_matches(inyon_core::js::text::is_space);
    if token.len() == rest.len() || token.is_empty() || token.chars().any(inyon_core::js::text::is_space) {
        return None;
    }
    Some(token.to_owned())
}

/// The companion a request comes from, by its bearer token.
pub fn companion_for(req: &Req) -> Option<CompanionRecord> {
    let token = bearer(req)?;
    let hash = sha256(&token);
    let now = now_ms();
    let (c, seen) = with(|s| {
        let c = s.companions.iter_mut().find(|c| same(&c.token_hash, &hash))?;
        let seen = now.saturating_sub(c.last_seen) >= SEEN_EVERY;
        if seen {
            c.last_seen = now;
        }
        Some((c.clone(), seen))
    })?;
    if seen {
        save();
    }
    Some(c)
}

/// 426 unless the request names a contract this hub speaks, in its header or (a WebSocket) `?contract=`.
pub fn contract_refusal(req: &Req, from_query: Option<String>) -> Option<Resp> {
    let raw = req.header("x-inyon-contract").or(from_query).unwrap_or_default();
    let given = raw.jtrim();
    if CONTRACTS.iter().any(|v| v.to_string() == given) {
        return None;
    }
    let list = CONTRACTS.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    let error = if given.is_empty() {
        format!("Send the X-Inyon-Contract header. This hub supports companion contract {list}.")
    } else {
        format!("This hub doesn't support companion contract {}. It supports {list}.", given.slice16(0, Some(20)))
    };
    Some(answer_json(426, &json!({ "error": error, "reason": "contract", "supported": CONTRACTS })))
}

pub const SCOPE_REFUSED: &str = "Companion apps can't use this";

pub type CompanionRun = Rc<dyn Fn(Req, CompanionRecord) -> LocalBoxFuture<'static, HubResult<Resp>>>;

/// What a companion may call on a route, and with which scope; every other route refuses it.
#[derive(Clone)]
pub struct CompanionRoute {
    pub scope: Option<CompanionScope>,
    pub run: CompanionRun,
}

fn route<F, Fut>(scope: Option<CompanionScope>, f: F) -> CompanionRoute
where
    F: Fn(Req, CompanionRecord) -> Fut + 'static,
    Fut: std::future::Future<Output = HubResult<Resp>> + 'static,
{
    CompanionRoute { scope, run: Rc::new(move |r, c| Box::pin(f(r, c))) }
}

/// A companion's request on a route: 426 for a contract this hub doesn't speak, 403 for a route
/// (or a scope) that isn't its, else the route's companion handler.
pub async fn as_companion(req: Req, c: CompanionRecord, route: Option<CompanionRoute>) -> Resp {
    if let Some(r) = contract_refusal(&req, None) {
        return r;
    }
    let Some(route) = route.filter(|r| r.scope.is_none_or(|s| c.scopes.contains(&s))) else {
        return refuse(403, "scope", SCOPE_REFUSED);
    };
    crate::http::answer((route.run)(req, c).await)
}

/// `GET /api/companions/me`
pub fn me(c: &CompanionRecord) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(c.id));
    m.insert("name".into(), json!(c.name));
    if let Some(a) = &c.app {
        m.insert("app".into(), json!(a));
    }
    m.insert("scopes".into(), json!(c.scopes));
    m.insert("contractVersion".into(), json!(c.contract_version));
    m.insert("hub".into(), crate::phones::hub_about());
    Value::Object(m)
}

// ---------- answer and send ----------

/// Why a pane can't be acted on now: gone, or its machine isn't reachable.
fn pane_problem(host_id: &str, pane_id: &str) -> Option<Resp> {
    let Some(rt) = registry().get(host_id).filter(|_| registry().find_pane(host_id, pane_id).is_some()) else {
        return Some(refuse(409, "pane-gone", "That session is gone."));
    };
    let rt = rt.borrow();
    if rt.state.status != HostStatus::Online {
        return Some(refuse(409, "offline", &format!("{} isn't reachable right now.", rt.host.config().name)));
    }
    None
}

const ALREADY: &str = "That question was already answered, or is no longer on screen.";

/// An answer to the question the companion saw, by its key: anything else on screen now is
/// "already answered".
async fn companion_answer(mut req: Req, c: CompanionRecord) -> HubResult<Resp> {
    let b = req.body().await?;
    let host = req.param("id");
    let pane = pane_param(&req.param("pane"));
    let Some(key) = b.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).map(str::to_owned) else {
        return Ok(refuse(400, "bad-request", "Send the prompt key from chat.screen with the answer."));
    };
    let option = b.get("option").and_then(|v| v.as_f64()).filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && b.get("option").is_some_and(Value::is_number));
    let Some(option) = option else {
        return Ok(refuse(400, "bad-request", "option must be the number of a choice."));
    };
    let text = b.get("text").and_then(Value::as_str).map(str::to_owned);
    if let Some(r) = pane_problem(&host, &pane) {
        return Ok(r);
    }
    let screen = actions::current_screen(&host, &pane).await?;
    let Some(p) = screen.prompt.filter(|p| prompt_key(&p.body, &p.options) == key) else {
        return Ok(refuse(409, "already-answered", ALREADY));
    };
    let offered = if option == 0.0 { p.confirm.as_deref().is_some_and(|c| !c.is_empty()) } else { p.options.iter().any(|o| o.n as f64 == option) };
    if !offered {
        return Ok(refuse(400, "bad-option", &format!("That question has no option {}.", inyon_core::js::json::number(option))));
    }
    // an answer picks a choice; typed words go only into the question's own free-text choice, and
    // they're text sent to the agent, so they need the send scope and its rules
    let typed = text.filter(|t| !t.jtrim().is_empty());
    if let Some(t) = &typed {
        if p.free_text_option.map(|n| n as f64) != Some(option) {
            return Ok(refuse(400, "bad-request", "Text goes only with the question’s free-text choice."));
        }
        if !c.scopes.contains(&CompanionScope::Send) {
            return Ok(refuse(403, "scope", "Typing an answer needs the send permission."));
        }
        if let Some(r) = text_refusal(t) {
            return Ok(r);
        }
    }
    match actions::answer(&host, &pane, option as u32, typed.as_deref(), Some(&key)).await {
        Ok(()) => Ok(ok_json(&json!({ "ok": true }))),
        Err(e) if e.status == Some(409) => Ok(refuse(409, "already-answered", ALREADY)),
        Err(e) if e.status == Some(404) => Ok(refuse(409, "pane-gone", "That session is gone.")),
        Err(e) => Err(e),
    }
}

/// Why a companion's text can't be typed into a session, or None when it can: only printable
/// text, tabs and line breaks (no C0 control but those, no DEL or C1: they work as keys, Ctrl-U,
/// Esc, a paste's end), and no line that starts with / or ! (an agent's command, a shell).
pub fn text_refusal(text: &str) -> Option<Resp> {
    if inyon_core::re!(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]").is_match(text) {
        return Some(refuse(400, "bad-request", "Companion apps send printable text: no control characters or escape sequences."));
    }
    if text.split(['\r', '\n']).any(|line| line.jtrim_start().starts_with('/') || line.jtrim_start().starts_with('!')) {
        return Some(refuse(403, "scope", "Companion apps send plain text, not / or ! commands."));
    }
    None
}

/// Why the hub's send didn't land, from what it said: the same messages in both hubs.
pub fn send_reason(message: &str) -> &'static str {
    if inyon_core::re!(r"(?i)waiting for an answer").is_match(message) {
        "prompt-open"
    } else if inyon_core::re!(r"(?i)No agent is running").is_match(message) {
        "no-agent"
    } else if inyon_core::re!(r"(?i)input is not visible|when it finishes working").is_match(message) {
        "busy"
    } else {
        "not-landed"
    }
}

/// Plain text into a session, closed-loop: 200 says how far it got, 409 why it didn't land.
async fn companion_send(mut req: Req, c: CompanionRecord) -> HubResult<Resp> {
    let b = req.body().await?;
    let host = req.param("id");
    let pane = pane_param(&req.param("pane"));
    let Some(text) = b.get("text").and_then(Value::as_str).filter(|t| !t.jtrim().is_empty()).map(str::to_owned) else {
        return Ok(refuse(400, "bad-request", "Message is empty"));
    };
    if let Some(r) = text_refusal(&text) {
        return Ok(r);
    }
    let request_id = match b.get("requestId") {
        None => None,
        Some(Value::String(s)) if inyon_core::re!(r"^[\w-]{1,128}$").is_match(s) => Some(s.clone()),
        Some(_) => return Ok(refuse(400, "bad-request", "Invalid send id")),
    };
    if let Some(r) = pane_problem(&host, &pane) {
        return Ok(r);
    }
    if registry().find_pane(&host, &pane).and_then(|f| f.pane.agent).is_none() {
        return Ok(refuse(409, "no-agent", "No agent is running in this pane"));
    }
    let id = match &request_id {
        Some(r) => format!("companion-{r}"),
        None => format!("companion-{}", inyon_core::ids::random_uuid()),
    };
    let sent = match &request_id {
        Some(rid) => {
            let key = inyon_core::js::stringify(&json!(["companion", c.id, host, pane, rid]));
            let (h, p, t, i) = (host.clone(), pane.clone(), text.clone(), id.clone());
            send_once(&key, &text, move || {
                Box::pin(async move {
                    actions::send_message(&h, &p, &t, false, Some(&i), false, false).await?;
                    Ok(Value::Null)
                })
            })
            .await
            .map(|_| ())
        }
        None => actions::send_message(&host, &pane, &text, false, Some(&id), false, false).await,
    };
    if let Err(e) = sent {
        return Ok(match e.status {
            Some(404) => refuse(409, "pane-gone", "That session is gone."),
            Some(400) => refuse(400, "bad-request", &e.message),
            Some(409) => refuse(409, send_reason(&e.message), &e.message),
            _ => refuse(409, "not-landed", &e.message),
        });
    }
    let state = watchers().peek(&host, &pane).and_then(|w| w.delivery(&id)).map(|d| d.state);
    let state = match state {
        Some(SendStatus::Delivered) => "delivered",
        Some(SendStatus::Queued) => "queued",
        _ => "sent",
    };
    Ok(ok_json(&json!({ "ok": true, "state": state })))
}

/// The companion handler for a route of the shared table, by its path and method.
pub fn route_for(pattern: &str, method: &Method) -> Option<CompanionRoute> {
    match (pattern, method.as_str()) {
        ("/api/overview", "GET") => Some(route(Some(CompanionScope::Overview), |r, _c| async move {
            let mut hosts = serde_json::to_value(inyon_protocol::compat::hosts_for(&registry().overview(), r.catalog())).unwrap_or_default();
            companion_hosts(&mut hosts);
            Ok(ok_json(&json!({ "hosts": hosts })))
        })),
        ("/api/hosts/:id/panes/:pane/answer", "POST") => Some(route(Some(CompanionScope::Answer), companion_answer)),
        ("/api/hosts/:id/panes/:pane/send", "POST") => Some(route(Some(CompanionScope::Send), companion_send)),
        _ => None,
    }
}

/// `GET /api/companions/me`'s companion side.
/// `POST /api/companions/claim`: a companion hands in its code; the answer carries its token.
pub async fn claim(mut req: Req) -> HubResult<Resp> {
    if let Some(refused) = contract_refusal(&req, None) {
        return Ok(refused);
    }
    let b = req.json_body(8 * 1024).await?;
    let client = crate::auth::client_address(&req.ip(), req.header("x-forwarded-for").as_deref());
    let res = claim_companion(&client, &b, now_ms());
    // a wrong code costs a moment, as wrong phone digits do
    if res.status().as_u16() == 401 {
        inyon_core::platform::rt::sleep(400).await;
    }
    Ok(res)
}

/// The claim is open, and a companion that still holds an older token may pair again with a new code.
pub fn claim_route() -> CompanionRoute {
    route(None, |r, _c| claim(r))
}

pub fn me_route() -> CompanionRoute {
    route(None, |_r, c| async move { Ok(ok_json(&me(&c))) })
}

// ---------- the WebSocket ----------

/// Why a companion's frame is refused (the socket closes with 4403 and this), or None when it may go on.
pub fn inbound_refusal(raw: &str, scopes: &[CompanionScope]) -> Option<String> {
    let t = inyon_core::js::json::parse(raw).and_then(|m| m.get("t").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default();
    let need = match t.as_str() {
        "ping" | "quiet" => Some(None),
        "sub.overview" => Some(Some(CompanionScope::Overview)),
        "sub.chat" | "unsub.chat" => Some(Some(CompanionScope::Chat)),
        _ => None,
    };
    if let Some(need) = need {
        if need.is_none_or(|s| scopes.contains(&s)) {
            return None;
        }
    }
    let shown = t.slice16(0, Some(40)).to_owned();
    Some(format!("Not for companion apps: {}", if shown.is_empty() { "an unreadable frame".to_owned() } else { shown }))
}

/// Machines as a companion sees them: who each one is, never how the hub reaches it (SSH target,
/// key file, relays).
/// The overview as a companion gets it: enough to list sessions and label them. Who each machine
/// is, never how the hub reaches it (SSH target, key file, relays); no folders, transcripts,
/// process ids or the agent's numbers.
pub const COMPANION_HOST: [&str; 3] = ["config", "status", "sessions"];
pub const COMPANION_SESSION: [&str; 3] = ["id", "name", "windows"];
pub const COMPANION_WINDOW: [&str; 5] = ["id", "index", "name", "active", "panes"];
pub const COMPANION_PANE: [&str; 8] = ["id", "index", "active", "title", "label", "dead", "agent", "companion"];
pub const COMPANION_AGENT: [&str; 7] = ["kind", "custom", "state", "mode", "waitingFor", "options", "activity"];

pub fn companion_hosts(hosts: &mut Value) {
    shape_hosts(hosts, &COMPANION_PANE)
}

/// The overview cut down to what a client outside the app needs, with these fields on each pane.
pub fn shape_hosts(hosts: &mut Value, pane_keys: &[&str]) {
    // each level picked, then the level below it in place
    fn each(v: &mut Map<String, Value>, key: &str, keys: &[&str], inner: impl Fn(&mut Map<String, Value>)) {
        for item in v.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
            let mut picked = pick(item, keys);
            inner(&mut picked);
            *item = Value::Object(picked);
        }
    }
    let Some(list) = hosts.as_array_mut() else { return };
    for h in list {
        let mut host = pick(h, &COMPANION_HOST);
        let c = &h["config"];
        host.insert("config".into(), json!({ "id": c["id"], "name": c["name"], "kind": c["kind"] }));
        each(&mut host, "sessions", &COMPANION_SESSION, |s| {
            each(s, "windows", &COMPANION_WINDOW, |w| {
                each(w, "panes", pane_keys, |p| {
                    if let Some(a) = p.get("agent").filter(|a| a.is_object()) {
                        let a = Value::Object(pick(a, &COMPANION_AGENT));
                        p.insert("agent".into(), a);
                    }
                })
            })
        });
        *h = Value::Object(host);
    }
}

fn pick(v: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter().filter_map(|k| v.get(*k).map(|x| ((*k).to_owned(), x.clone()))).collect()
}

/// A chat's details as a companion gets them: no drafts, queue, send texts, links or paths.
pub const COMPANION_INFO: [&str; 10] = ["hostId", "paneId", "agent", "title", "model", "mode", "running", "status", "turn", "sends"];
pub const COMPANION_SEND: [&str; 3] = ["id", "state", "at"];
/// A chat's screen as a companion gets it: the question and the agent's state, not the input box's text.
pub const COMPANION_SCREEN: [&str; 10] = ["inputVisible", "prompt", "working", "activity", "since", "mode", "model", "effort", "dialog", "width"];

pub fn companion_info(info: &inyon_protocol::ChatInfo) -> Value {
    let all = serde_json::to_value(info).unwrap_or_default();
    let mut v = pick(&all, &COMPANION_INFO);
    if let Some(sends) = all.get("sends").and_then(Value::as_array) {
        v.insert("sends".into(), Value::Array(sends.iter().map(|s| Value::Object(pick(s, &COMPANION_SEND))).collect()));
    }
    Value::Object(v)
}

pub fn companion_screen(screen: &inyon_protocol::ScreenState) -> Value {
    let all = serde_json::to_value(screen).unwrap_or_default();
    let mut v = pick(&all, &COMPANION_SCREEN);
    if let (Some(p), Some(Value::Object(prompt))) = (&screen.prompt, v.get_mut("prompt")) {
        prompt.insert("key".into(), json!(prompt_key(&p.body, &p.options)));
    }
    Value::Object(v)
}

/// What a companion gets of a message to the app: the overview, notices, a chat's info and screen
/// (its question with the key to answer it by), the companion agent's replies, and pong. Chat history, live text, drafts and
/// terminal data never go to a companion.
pub fn companion_frame(msg: &ServerMsg, scopes: &[CompanionScope]) -> Option<Value> {
    let can = |s: CompanionScope| scopes.contains(&s);
    let whole = || serde_json::to_value(msg).ok();
    match msg {
        ServerMsg::Pong => whole(),
        ServerMsg::Overview { .. } if can(CompanionScope::Overview) => {
            let mut v = whole()?;
            companion_hosts(&mut v["hosts"]);
            // pins, tags and notes are the app's own
            if let Some(o) = v.as_object_mut() {
                o.remove("organize");
            }
            Some(v)
        }
        ServerMsg::Notify { .. } if can(CompanionScope::Overview) => whole(),
        ServerMsg::ChatInfo { key, info } | ServerMsg::ChatReset { key, info, .. } if can(CompanionScope::Chat) => {
            Some(json!({ "t": "chat.info", "key": key, "info": companion_info(info) }))
        }
        // the companion agent's replies, to every companion that may watch chats, subscribed or not
        ServerMsg::CompanionReply { .. } if can(CompanionScope::Chat) => whole(),
        ServerMsg::ChatScreen { key, screen } if can(CompanionScope::Chat) => Some(json!({ "t": "chat.screen", "key": key, "screen": companion_screen(screen) })),
        _ => None,
    }
}

// ---------- the list and removal ----------

/// Keeps `close` as a companion's connection until it ends (call the returned function), so
/// removing the companion can end it.
pub fn hold_companion_socket(id: &str, close: Closer) -> impl Fn() {
    let n = with(|s| {
        s.next += 1;
        s.sockets.push((s.next, close, id.to_owned()));
        s.next
    });
    move || with(|s| s.sockets.retain(|(i, _, _)| *i != n))
}

pub fn list_companions() -> Value {
    let all = with(|s| s.companions.clone());
    Value::Array(
        all.into_iter()
            .map(|c| {
                let mut m = Map::new();
                m.insert("id".into(), json!(c.id));
                m.insert("name".into(), json!(c.name));
                if let Some(a) = c.app {
                    m.insert("app".into(), json!(a));
                }
                m.insert("scopes".into(), json!(c.scopes.iter().map(|s| scope_name(*s)).collect::<Vec<_>>()));
                m.insert("contractVersion".into(), json!(c.contract_version));
                m.insert("pairedAt".into(), json!(c.paired_at));
                m.insert("lastSeen".into(), json!(c.last_seen));
                Value::Object(m)
            })
            .collect(),
    )
}

/// Forgets a companion: its token stops working now, and its connections are closed.
pub fn revoke_companion(id: &str) -> bool {
    if !with(|s| s.companions.iter().any(|c| c.id == id)) {
        return false;
    }
    with(|s| s.companions.retain(|c| c.id != id));
    save();
    let closing: Vec<Closer> = with(|s| {
        let (go, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut s.sockets).into_iter().partition(|(_, _, c)| c == id);
        s.sockets = keep;
        go.into_iter().map(|(_, c, _)| c).collect()
    });
    for c in closing {
        c(4001, "Companion removed");
    }
    true
}

#[cfg(test)]
#[path = "companions_tests.rs"]
mod tests;
