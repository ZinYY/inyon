//! Port of push/registrations.ts: the browsers that asked for notifications while they're closed.
//! A hub sends to them itself (inyon-hub's push); the phone app, which has no server, hands its own
//! to every paired computer, and each computer sends when its agents need you (inyon-phone).

use std::cell::RefCell;
use std::rc::Rc;

use inyon_core::clock::now_ms;
use inyon_core::hosts::http_error::{http, HubResult};
use inyon_core::platform::console;
use inyon_core::platform::store::kv;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

/// The browser's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: Keys,
}

/// A P-256 private key as JWK, as the browser exports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcJwk {
    pub kty: String,
    pub crv: String,
    pub d: String,
    pub x: String,
    pub y: String,
}

/// The browser's VAPID key pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vapid {
    #[serde(rename = "privateJwk")]
    pub private_jwk: EcJwk,
    /// who to contact about these pushes: a mailto: or https: address (Apple requires one)
    pub subject: String,
}

/// A browser that asked for notifications while it's closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    pub subscription: Subscription,
    pub vapid: Vapid,
    /// notices when an agent finishes too, not only when it needs you
    pub done: bool,
    /// notices when a page's board moves on (from 0.22.4; on for a registration from before)
    #[serde(default = "yes")]
    pub pages: bool,
    pub at: u64,
}

fn yes() -> bool {
    true
}

fn key(endpoint: &str) -> String {
    format!("push/{}", &crate::http::sha256(endpoint)[..32])
}

pub fn registrations() -> Vec<Registration> {
    kv::keys("push/").iter().filter_map(|k| kv::get::<Registration>(k)).filter(|r| !r.subscription.endpoint.is_empty()).collect()
}

/// Checks what a browser sent: an https endpoint, its two keys, and a P-256 private key.
pub fn valid_registration(b: &Value) -> HubResult<Registration> {
    let bad = |what: &str| http(400, format!("Bad push registration: {what}"));
    let s = b.get("subscription");
    let str_at = |v: Option<&Value>, k: &str| v.and_then(|v| v.get(k)).and_then(Value::as_str).map(str::to_owned);
    let endpoint = str_at(s, "endpoint").filter(|e| e.starts_with("https://") && e.encode_utf16().count() <= 2048).ok_or_else(|| bad("endpoint"))?;
    let keys = s.and_then(|s| s.get("keys"));
    let (Some(p256dh), Some(auth)) = (str_at(keys, "p256dh"), str_at(keys, "auth")) else { return Err(bad("keys")) };
    let jwk = b.get("vapid").and_then(|v| v.get("privateJwk"));
    let jwk_ok = str_at(jwk, "kty").as_deref() == Some("EC") && str_at(jwk, "crv").as_deref() == Some("P-256");
    let (Some(d), Some(x), Some(y)) = (str_at(jwk, "d"), str_at(jwk, "x"), str_at(jwk, "y")) else { return Err(bad("VAPID key")) };
    if !jwk_ok {
        return Err(bad("VAPID key"));
    }
    let subject = crate::http::str_or_empty(b.get("vapid").and_then(|v| v.get("subject")));
    if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
        return Err(bad("VAPID subject"));
    }
    Ok(Registration {
        subscription: Subscription { endpoint, keys: Keys { p256dh, auth } },
        vapid: Vapid { private_jwk: EcJwk { kty: "EC".into(), crv: "P-256".into(), d, x, y }, subject },
        done: b.get("done") != Some(&Value::Bool(false)),
        pages: b.get("pages") != Some(&Value::Bool(false)),
        at: now_ms(),
    })
}

/// How many browsers a hub sends to. Each one is a URL the hub POSTs to whenever an agent needs
/// you, and only signed-in browsers and phones may add one, but there's no reason for the list to
/// grow without end: past this, the oldest goes.
pub const MAX_REGISTRATIONS: usize = 20;

pub fn add_registration(r: &Registration) {
    if let Err(e) = kv::set(&key(&r.subscription.endpoint), r) {
        console::error(&format!("push: couldn't save a registration: {e}"));
    }
    let mut all = registrations();
    all.sort_by_key(|r| r.at);
    for old in all.iter().take(all.len().saturating_sub(MAX_REGISTRATIONS)) {
        kv::del(&key(&old.subscription.endpoint));
    }
    changed();
}

pub fn remove_registration(endpoint: &str) {
    kv::del(&key(endpoint));
    changed();
}

type Listener = Rc<dyn Fn()>;

thread_local! {
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
}

/// Called after each registration added or removed.
pub fn on_registrations(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Rc::new(f)));
}

fn changed() {
    for l in LISTENERS.with(|l| l.borrow().clone()) {
        l();
    }
}
