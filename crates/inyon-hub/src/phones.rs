//! Port of phones.ts: phones paired with this hub. The hub's web app, signed in, shows a one-time
//! code (a QR, or an ID and six digits to type); the phone claims it and gets a token it sends as a
//! bearer from then on, since a browser cookie never leaves the browser. Codes and tickets live in
//! memory, so a restart drops them; paired phones are kept in the records, with only the hash of
//! each token.

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;
use inyon_core::clock::now_ms;
use inyon_core::hosts::http_error::{http, HubResult};
use inyon_core::js::JsStr;
use inyon_core::platform::store::kv;
use inyon_link::code::{new_short_code, parse_short_code};
use inyon_protocol::VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::Closer;
use crate::http::{random_base64url, random_hex, same, sha256, Req};

/// how long a code stays claimable
pub const CODE_TTL: u64 = 10 * 60_000;
/// how long a WebSocket ticket stays usable
pub const TICKET_TTL: u64 = 30_000;
/// wrong digits before a code is dropped: six digits give a guess 5 in a million
const WRONG_TRIES: u32 = 5;
/// failed claims one address gets a minute, as with the setup code
const CLAIMS_PER_MINUTE: u32 = 8;
/// how often a phone's lastSeen is written
const SEEN_EVERY: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneRecord {
    pub id: String,
    pub name: String,
    /// sha256 of the token, hex
    pub token_hash: String,
    pub paired_at: u64,
    pub last_seen: u64,
}

struct Code {
    code: String,
    expires_at: u64,
    tries: u32,
}

#[derive(Default)]
struct State {
    phones: Vec<PhoneRecord>,
    codes: IndexMap<String, Code>,
    claims: HashMap<String, (u32, u64)>,
    tickets: HashMap<String, (String, u64)>,
    sockets: Vec<(u64, Closer, String)>,
    next: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Reads the paired phones from the records (the module's top level in the TS).
pub fn init() {
    let phones: Vec<PhoneRecord> = kv::get("phones").unwrap_or_default();
    with(|s| s.phones = phones);
}

fn save_phones() {
    let all = with(|s| s.phones.clone());
    if let Err(e) = kv::set("phones", &all) {
        inyon_core::platform::console::error(&format!("couldn't save the paired phones: {e}"));
    }
}

/// The hub as the app names it; INYON_HUB_NAME for a name of your own.
pub fn hub_name() -> String {
    std::env::var("INYON_HUB_NAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(inyon_engine::hostname)
}

/// A stable id for this hub, made once, so the app tells two hubs apart even when both get renamed.
pub fn hub_id() -> String {
    if let Some(id) = kv::get::<String>("hub.id").filter(|s| !s.is_empty()) {
        return id;
    }
    let id = random_hex(8);
    let _ = kv::set("hub.id", &id);
    id
}

/// What a phone shows for this hub; sent again with every ticket, so a renamed or updated hub shows as it is now.
pub fn hub_about() -> Value {
    json!({ "id": hub_id(), "name": hub_name(), "version": VERSION })
}

// ---------- pairing ----------

/// The hub address the phone will talk to; the page sends its own origin, so a proxy's name is kept.
fn hub_origin(origin: Option<&Value>) -> HubResult<String> {
    let raw = crate::http::str_or_empty(origin);
    let s = raw.trim_end_matches('/');
    let ok = url::Url::parse(s).ok().filter(|u| (u.scheme() == "http" || u.scheme() == "https") && u.origin().ascii_serialization() == s);
    ok.map(|_| s.to_owned()).ok_or_else(|| http(400, "Unexpected hub address"))
}

/// A fresh code for the web app to show.
/// What the QR holds: inyon://hub?u=<origin>&c=<id:code>&n=<name>&h=<hub id>, and f=<sha256> when
/// the hub serves HTTPS itself. The phone shows the id and the certificate next to the name, which
/// anyone can write into a link.
pub fn pair_link(hub: &str, code: &str, fingerprint: Option<&str>) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("u", hub).append_pair("c", code).append_pair("n", &hub_name()).append_pair("h", &hub_id());
    if let Some(fp) = fingerprint {
        q.append_pair("f", fp);
    }
    format!("inyon://hub?{}", q.finish())
}

pub fn new_pair_code(origin: Option<&Value>, now: u64) -> HubResult<Value> {
    let hub = hub_origin(origin)?;
    let c = new_short_code().map_err(|e| http(500, e.to_string()))?;
    let expires_at = now + CODE_TTL;
    with(|s| {
        s.codes.retain(|_, c| c.expires_at > now);
        s.codes.insert(c.id.clone(), Code { code: c.code.clone(), expires_at, tries: 0 });
    });
    let link = pair_link(&hub, &format!("{}:{}", c.id, c.code), crate::tls::served_sha256().as_deref());
    Ok(json!({ "id": c.id, "code": c.code, "link": link, "expiresAt": expires_at }))
}

const EXPIRED: &str = "That code has expired. Show a new one on the hub.";

fn count_failure(s: &mut State, ip: &str, now: u64) {
    s.claims.retain(|_, (_, reset)| *reset >= now);
    let a = s.claims.entry(ip.to_owned()).or_insert((0, now + 60_000));
    a.0 += 1;
}

/// The phone hands in a code. Unknown and expired codes read the same, so the ID alone tells a
/// guesser nothing; wrong digits are counted against the code and against the address.
pub fn claim_phone(ip: &str, claim: &Value, now: u64) -> HubResult<Value> {
    let limited = with(|s| s.claims.get(ip).is_some_and(|(n, reset)| *reset >= now && *n >= CLAIMS_PER_MINUTE));
    if limited {
        return Err(http(429, "Too many attempts. Try again in a minute."));
    }
    let code = crate::http::str_or_empty(claim.get("code"));
    let parsed = parse_short_code(&code);
    with(|s| {
        let entry_ok = parsed.as_ref().and_then(|p| s.codes.get(&p.id)).is_some_and(|e| e.expires_at > now);
        let Some(p) = parsed.as_ref().filter(|_| entry_ok) else {
            if let Some(p) = &parsed {
                s.codes.shift_remove(&p.id);
            }
            count_failure(s, ip, now);
            return Err(http(404, EXPIRED));
        };
        let entry = s.codes.get_mut(&p.id).expect("checked above");
        if !same(&p.code, &entry.code) {
            entry.tries += 1;
            if entry.tries >= WRONG_TRIES {
                s.codes.shift_remove(&p.id);
            }
            count_failure(s, ip, now);
            return Err(http(401, "Those digits are not right."));
        }
        s.codes.shift_remove(&p.id);
        Ok(())
    })?;
    let token = random_base64url(32);
    let raw_name = crate::http::str_or_empty(claim.get("name"));
    let name: String = raw_name.jtrim().slice16(0, Some(60)).to_owned();
    let name = if name.is_empty() { "Phone".to_owned() } else { name };
    with(|s| s.phones.push(PhoneRecord { id: random_hex(6), name, token_hash: sha256(&token), paired_at: now, last_seen: now }));
    save_phones();
    Ok(json!({ "token": token, "hub": hub_about() }))
}

// ---------- signed-in phones ----------

/// `req.headers.get('authorization')?.match(/^Bearer\s+(\S+)$/i)?.[1]`
pub(crate) fn bearer(req: &Req) -> Option<String> {
    let h = req.header("authorization")?;
    let rest = h.get(..6).filter(|p| p.eq_ignore_ascii_case("bearer")).map(|_| &h[6..])?;
    let token = rest.trim_start_matches(inyon_core::js::text::is_space);
    if token.len() == rest.len() || token.is_empty() || token.chars().any(inyon_core::js::text::is_space) {
        return None;
    }
    Some(token.to_owned())
}

/// The request carries a bearer token (good or not): it is a phone's, never a browser's.
pub fn has_bearer(req: &Req) -> bool {
    bearer(req).is_some()
}

fn by_token(token: &str, now: u64) -> Option<PhoneRecord> {
    let hash = sha256(token);
    let (phone, seen) = with(|s| {
        let p = s.phones.iter_mut().find(|p| same(&p.token_hash, &hash))?;
        let seen = now.saturating_sub(p.last_seen) >= SEEN_EVERY;
        if seen {
            p.last_seen = now;
        }
        Some((p.clone(), seen))
    })?;
    if seen {
        save_phones();
    }
    Some(phone)
}

/// The paired phone a request comes from, by its bearer token.
pub fn phone_for(req: &Req) -> Option<PhoneRecord> {
    phone_for_at(req, now_ms())
}

pub fn phone_for_at(req: &Req, now: u64) -> Option<PhoneRecord> {
    bearer(req).and_then(|t| by_token(&t, now))
}

/// Whether a phone with this id is still paired (a page link a phone made stops working once it isn't).
pub fn phone_paired(id: &str) -> bool {
    with(|s| s.phones.iter().any(|p| p.id == id))
}

// ---------- WebSocket tickets ----------

// a page can't put a header on a WebSocket, so a phone trades its token for a short-lived ticket
pub fn issue_ticket(phone_id: &str, now: u64) -> String {
    let ticket = random_base64url(32);
    with(|s| {
        s.tickets.retain(|_, (_, exp)| *exp > now);
        s.tickets.insert(ticket.clone(), (phone_id.to_owned(), now + TICKET_TTL));
    });
    ticket
}

/// The phone a ticket was issued to, once: a second use finds nothing.
pub fn redeem_ticket(ticket: &str, now: u64) -> Option<PhoneRecord> {
    with(|s| {
        let (phone, exp) = s.tickets.remove(ticket)?;
        if exp <= now {
            return None;
        }
        s.phones.iter().find(|p| p.id == phone).cloned()
    })
}

// ---------- open connections and removal ----------

/// Keeps `close` as a phone's connection until it ends (call the returned function), so removing
/// the phone can end it.
pub fn hold_phone_socket(phone_id: &str, close: Closer) -> impl Fn() {
    let id = with(|s| {
        s.next += 1;
        s.sockets.push((s.next, close, phone_id.to_owned()));
        s.next
    });
    move || with(|s| s.sockets.retain(|(i, _, _)| *i != id))
}

/// Lists the phones for the app; the one asking is marked.
pub fn list_phones(req: &Req) -> Value {
    list_phones_at(req, now_ms())
}

pub fn list_phones_at(req: &Req, now: u64) -> Value {
    let mine = phone_for_at(req, now).map(|p| p.id);
    let all = with(|s| s.phones.clone());
    Value::Array(
        all.into_iter()
            .map(|p| {
                let mut v = json!({ "id": p.id, "name": p.name, "pairedAt": p.paired_at, "lastSeen": p.last_seen });
                if mine.as_deref() == Some(p.id.as_str()) {
                    v["thisPhone"] = json!(true);
                }
                v
            })
            .collect(),
    )
}

/// Forgets a phone: its token stops working now, and its connections are closed.
pub fn revoke_phone(id: &str) -> bool {
    let known = with(|s| s.phones.iter().any(|p| p.id == id));
    if !known {
        return false;
    }
    with(|s| s.phones.retain(|p| p.id != id));
    save_phones();
    let closing: Vec<Closer> = with(|s| {
        let (go, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut s.sockets).into_iter().partition(|(_, _, p)| p == id);
        s.sockets = keep;
        go.into_iter().map(|(_, c, _)| c).collect()
    });
    for c in closing {
        c(4001, "Phone removed");
    }
    true
}

#[cfg(test)]
#[path = "phones_tests.rs"]
mod tests;
