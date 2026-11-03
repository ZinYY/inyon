//! Port of auth.ts: the browser's sign-in. A session is a signed cookie (HMAC-SHA256 under the
//! hub's secret) holding its expiry and a nonce; signing out puts the nonce on a revoked list kept
//! in the records until the session would have ended anyway. Guesses at the password are slowed
//! per client and for all clients together.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use hmac::{KeyInit, Mac};
use indexmap::IndexMap;
use inyon_core::clock::now_ms;
use inyon_core::platform::store::kv;
use serde_json::{json, Map, Value};

use crate::http::{same, Req};
use crate::password::{check_password, session_secret};
use crate::phones::{has_bearer, phone_for};

pub const COOKIE: &str = "inyon_session";
/// The name over HTTPS. A `__Host-` cookie is only taken with Secure, Path=/ and no Domain, so a
/// service on a sibling subdomain or on plain http on the same host can't set one to shadow it.
/// Plain http (a VPN address) can't use it and keeps `inyon_session`; a request with both reads
/// this one first.
pub const SECURE_COOKIE: &str = "__Host-inyon_session";
/// the cookie's name before the rename to Inyon: still signs you in, and is swapped for the new one
const OLD_COOKIE: &str = "tether_session";
/// a session ends after 30 days without use; using it pushes the end back (see refresh_cookie)
const MAX_AGE: u64 = 60 * 60 * 24 * 30;
const REFRESH_AFTER_MS: u64 = 24 * 60 * 60 * 1000;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn sign(payload: &str) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(session_secret().as_bytes()).expect("an HMAC key");
    mac.update(payload.as_bytes());
    B64U.encode(mac.finalize().into_bytes())
}

/// What a cookie holds, in the order its keys were written (a refresh keeps that order).
#[derive(Debug, Clone)]
pub struct Session(Map<String, Value>);

impl Session {
    fn num(&self, k: &str) -> Option<f64> {
        self.0.get(k).and_then(Value::as_f64)
    }
    pub fn nonce(&self) -> String {
        self.0.get("n").map(crate::http::js_string).unwrap_or_default()
    }
    fn exp(&self) -> f64 {
        self.num("exp").unwrap_or(f64::NAN)
    }
}

/// a closer for an open connection, by an id of its own
pub type Closer = Rc<dyn Fn(u16, &str)>;

struct State {
    // signed-out sessions, by nonce, until they would have expired anyway
    revoked: IndexMap<String, f64>,
    // open connections, by the session they signed in with: a cookie is only checked when a
    // connection opens, so ending a session has to close the ones it already has
    sockets: Vec<(u64, Closer, String)>,
    next: u64,
    attempts: HashMap<String, (u32, u64)>,
    failures: (u32, u64),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { revoked: IndexMap::new(), sockets: vec![], next: 0, attempts: HashMap::new(), failures: (0, 0) });
}

/// Reads the revoked list from the records (the module's top level in the TS).
pub fn init() {
    let revoked: IndexMap<String, f64> = match kv::get_value("revoked") {
        Some(Value::Object(m)) => m.into_iter().filter_map(|(k, v)| v.as_f64().map(|x| (k, x))).collect(),
        _ => IndexMap::new(),
    };
    STATE.with(|s| s.borrow_mut().revoked = revoked);
}

fn save_revoked() {
    let now = now_ms() as f64;
    let all = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.revoked.retain(|_, exp| *exp >= now);
        s.revoked.iter().map(|(k, v)| (k.clone(), inyon_core::js::json::number(*v))).collect::<Map<String, Value>>()
    });
    if let Err(e) = kv::set_value("revoked", &Value::Object(all)) {
        inyon_core::platform::console::error(&format!("couldn't save the signed-out sessions: {e}"));
    }
}

fn cookie_for(session: &Map<String, Value>, secure: bool) -> String {
    let payload = B64U.encode(inyon_core::js::stringify(&Value::Object(session.clone())));
    let name = if secure { SECURE_COOKIE } else { COOKIE };
    format!("{name}={payload}.{}; Path=/; HttpOnly; SameSite=Lax; Max-Age={MAX_AGE}{}", sign(&payload), if secure { "; Secure" } else { "" })
}

pub fn issue_cookie(secure: bool) -> String {
    let now = now_ms();
    let s = json!({ "exp": now + MAX_AGE * 1000, "n": inyon_core::ids::random_uuid(), "iat": now, "at": now });
    cookie_for(s.as_object().expect("an object"), secure)
}

pub fn clear_cookie(secure: bool) -> String {
    if secure {
        format!("{SECURE_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Secure")
    } else {
        format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
    }
}

/// `raw.match(/(?:^|;\s*)name=([^;]+)/)?.[1]`
fn cookie_value(raw: &str, name: &str) -> Option<String> {
    let mut at = 0;
    while let Some(i) = raw[at..].find(&format!("{name}=")) {
        let start = at + i;
        let before = &raw[..start];
        let boundary = start == 0 || before.trim_end_matches(inyon_core::js::text::is_space).ends_with(';');
        let value_at = start + name.len() + 1;
        if boundary {
            let v: String = raw[value_at..].chars().take_while(|c| *c != ';').collect();
            if !v.is_empty() {
                return Some(v);
            }
        }
        at = start + 1;
    }
    None
}

fn credential(req: &Req) -> Option<String> {
    let raw = req.header("cookie").unwrap_or_default();
    cookie_value(&raw, SECURE_COOKIE).or_else(|| cookie_value(&raw, COOKIE)).or_else(|| cookie_value(&raw, OLD_COOKIE))
}

/// the cookie isn't under the name this request should use yet (an older name, or plain where HTTPS is)
fn old_cookie_only(req: &Req, secure: bool) -> bool {
    cookie_value(&req.header("cookie").unwrap_or_default(), if secure { SECURE_COOKIE } else { COOKIE }).is_none()
}

/// The session a request's cookie carries, if it's signed, current and not revoked.
pub fn session(req: &Req) -> Option<Session> {
    let value = credential(req)?;
    let mut parts = value.split('.');
    let payload = parts.next().filter(|p| !p.is_empty())?;
    let sig = parts.next().filter(|s| !s.is_empty())?;
    if !same(sig, &sign(payload)) {
        return None;
    }
    // Buffer.from(…, 'base64url') skips what isn't base64
    let clean: String = payload.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let bytes = B64U.decode(clean.trim_end_matches('=')).ok()?;
    let Some(Value::Object(m)) = inyon_core::js::json::parse(&String::from_utf8_lossy(&bytes)) else { return None };
    let s = Session(m);
    let revoked = STATE.with(|st| st.borrow().revoked.contains_key(&s.nonce()));
    (s.exp() > now_ms() as f64 && !revoked).then_some(s)
}

/// A browser with a session cookie, or a paired phone with its token. A request that carries a
/// bearer is judged by the bearer alone: it skips the same-origin check, so a cookie sent along
/// with a bad bearer must not sign it in.
pub fn is_authed(req: &Req) -> bool {
    if has_bearer(req) {
        phone_for(req).is_some()
    } else {
        session(req).is_some()
    }
}

/// The password was typed within `ms`: pairing a phone asks for that, not a session left from weeks ago.
pub fn recent_sign_in(req: &Req, ms: u64) -> bool {
    let Some(s) = session(req) else { return false };
    // only `at` says when the password was typed: a cookie from before it has just `iat`, which a
    // daily refresh moves to now, so it never counts as recent
    s.num("at").is_some_and(|at| now_ms() as f64 - at <= ms as f64)
}

/// Keeps `close` until the connection ends (call the returned function then), so signing out (or
/// a new password) can end it.
pub fn hold_socket(req: &Req, close: Closer) -> impl Fn() {
    let id = STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.next += 1;
        st.next
    });
    if let Some(s) = session(req) {
        STATE.with(|st| st.borrow_mut().sockets.push((id, close, s.nonce())));
    }
    move || STATE.with(|st| st.borrow_mut().sockets.retain(|(i, _, _)| *i != id))
}

/// Closes the connections of one session, or of every session when `nonce` is None.
pub fn close_sockets(nonce: Option<&str>) {
    let closing: Vec<Closer> = STATE.with(|st| {
        let mut st = st.borrow_mut();
        let (go, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.sockets).into_iter().partition(|(_, _, n)| nonce.is_none_or(|x| x == n));
        st.sockets = keep;
        go.into_iter().map(|(_, c, _)| c).collect()
    });
    for c in closing {
        c(4401, "Signed out");
    }
}

/// A fresh expiry for a session in use, at most once a day (same nonce, so sign-out still ends it).
pub fn refresh_cookie(req: &Req, secure: bool) -> Option<String> {
    let s = session(req)?;
    let iat = s.num("iat").unwrap_or(0.0);
    if (now_ms() as f64 - iat) < REFRESH_AFTER_MS as f64 && !old_cookie_only(req, secure) {
        return None;
    }
    let now = now_ms();
    let mut m = s.0;
    m.insert("exp".into(), json!(now + MAX_AGE * 1000));
    m.insert("iat".into(), json!(now));
    Some(cookie_for(&m, secure))
}

/// Signing out ends the session everywhere it was copied, not just in this browser.
pub fn revoke_session(req: &Req) {
    let Some(s) = session(req) else { return };
    let n = s.nonce();
    STATE.with(|st| st.borrow_mut().revoked.insert(n.clone(), s.exp()));
    save_revoked();
    close_sockets(Some(&n));
}

// Slow down password guessing: a few attempts per minute per address, and a cap on failures from
// all addresses together, past which addresses that never signed in are slowed right down
// (limits.rs), so guesses spread over many addresses can't go faster either.
const PER_CLIENT: u32 = 8;
const ALL_CLIENTS: u32 = 30;
/// what `try_login` answers when the password wasn't even checked
pub const TOO_MANY: &str = "Too many attempts. Try again in a minute.";

/// `tryLogin`: Ok, or the error the app shows (`TOO_MANY` when it wasn't checked).
pub async fn try_login(ip: &str, password: &str) -> Result<(), &'static str> {
    let now = now_ms();
    let (failures, over) = STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.attempts.retain(|_, (_, reset)| *reset >= now);
        if st.failures.1 < now {
            st.failures = (0, now + 60_000);
        }
        let failures = st.failures.0;
        (failures, st.attempts.get(ip).is_some_and(|a| a.0 >= PER_CLIENT))
    });
    let refused = over
        || !crate::limits::shared_cap_allows("password", ip, failures, ALL_CLIENTS, now)
        || STATE.with(|st| {
            let mut st = st.borrow_mut();
            let a = st.attempts.entry(ip.to_owned()).or_insert((0, now + 60_000));
            // counted before the slow hash, so guesses sent all at once are each counted
            a.0 += 1;
            st.failures.0 += 1;
            false
        });
    if refused {
        return Err(TOO_MANY);
    }
    if password.encode_utf16().count() <= 1024 && check_password(password).await {
        STATE.with(|st| {
            let mut st = st.borrow_mut();
            st.attempts.remove(ip);
            st.failures.0 = st.failures.0.saturating_sub(1);
        });
        crate::limits::remember_address(ip, now_ms());
        return Ok(());
    }
    inyon_core::platform::rt::sleep(400).await;
    Err("Wrong password")
}

/// The address to count sign-in attempts against. Behind a local proxy every request comes from
/// loopback and X-Forwarded-For names the client. Only its last entry, the one the proxy added,
/// can be trusted: a client can send any entries before it.
pub fn client_address(peer: &str, forwarded_for: Option<&str>) -> String {
    if !loopback(peer) {
        return peer.to_owned();
    }
    forwarded_for.and_then(|f| f.split(',').next_back()).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| peer.to_owned())
}

/// `/^(127\.|::1$|::ffff:127\.)/`
pub fn loopback(ip: &str) -> bool {
    ip.starts_with("127.") || ip == "::1" || ip.starts_with("::ffff:127.")
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
