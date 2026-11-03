//! Port of claude/send-receipts.ts: a send with an id goes to the agent once, however often the app
//! retries it (a lost response, a restart in between), without keeping the message itself.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::clock::now_ms;
use crate::hosts::http_error::{HubError, HubResult};
use crate::platform::rt::{spawn_task, Deferred};
use crate::platform::store::kv;
use crate::sha::sha256_hex;

const RETENTION_MS: u64 = 24 * 60 * 60_000;
const KEEP_ACCEPTED: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Accepted,
}

struct Receipt<T> {
    digest: String,
    expires: u64,
    state: State,
    result: Option<Deferred<HubResult<T>>>,
}

/// The receipts, by send id, in the order they came (`Map`).
pub struct SendReceipts<T> {
    receipts: Rc<RefCell<IndexMap<String, Receipt<T>>>>,
    key: Option<String>,
}

impl<T: Clone + 'static> SendReceipts<T> {
    /// `key` is where the receipts are kept; without one they live only in memory.
    pub fn new(key: Option<&str>) -> Self {
        let mut receipts = IndexMap::new();
        if let Some(k) = key {
            if let Some(Value::Object(saved)) = kv::get_value(k) {
                for (id, r) in saved {
                    let digest = r.get("digest").and_then(Value::as_str);
                    let expires = r.get("expires").and_then(Value::as_f64);
                    let state = match r.get("state").and_then(Value::as_str) {
                        Some("pending") => Some(State::Pending),
                        Some("accepted") => Some(State::Accepted),
                        _ => None,
                    };
                    if let (Some(digest), Some(expires), Some(state)) = (digest, expires, state) {
                        if expires > now_ms() as f64 {
                            receipts.insert(id, Receipt { digest: digest.to_owned(), expires: expires as u64, state, result: None });
                        }
                    }
                }
            }
        }
        SendReceipts { receipts: Rc::new(RefCell::new(receipts)), key: key.map(str::to_owned) }
    }

    fn save(receipts: &IndexMap<String, Receipt<T>>, key: Option<&str>) -> Result<(), String> {
        let Some(key) = key else { return Ok(()) };
        let all: Map<String, Value> = receipts
            .iter()
            .map(|(id, r)| (id.clone(), serde_json::json!({ "digest": r.digest, "expires": r.expires, "state": if r.state == State::Pending { "pending" } else { "accepted" } })))
            .collect();
        kv::set_value(key, &Value::Object(all))
    }

    /// Survives lost responses and restarts without retaining prompt text. The send's own answer,
    /// or None for a retry of one accepted before a restart (whose answer is gone).
    pub fn send_once(&self, key: &str, text: &str, send: impl FnOnce() -> LocalBoxFuture<'static, HubResult<T>> + 'static) -> LocalBoxFuture<'static, HubResult<Option<T>>> {
        let now = now_ms();
        let digest = sha256_hex(text);
        let fail = |m: &str| -> LocalBoxFuture<'static, HubResult<Option<T>>> {
            let e = HubError::plain(m);
            Box::pin(async move { Err(e) })
        };
        let mut all = self.receipts.borrow_mut();
        all.retain(|_, r| !(r.expires < now && (r.state == State::Accepted || r.result.is_none())));
        if let Some(existing) = all.get(key) {
            if existing.digest != digest {
                return fail("The send id was already used for a different message");
            }
            if let Some(r) = existing.result.clone() {
                return Box::pin(async move { r.wait().await.map(Some) });
            }
            if existing.state == State::Accepted {
                return Box::pin(async { Ok(None) });
            }
            return fail("The hub restarted during this send. Check the terminal before sending again. Your message is available to copy.");
        }
        // the cap is on sends still going; finished ones are kept a day only to answer retries, and
        // the oldest of those make way once there are many
        if all.values().filter(|r| r.state == State::Pending).count() >= 1000 {
            return fail("Too many outstanding sends. Try again later.");
        }
        let ids: Vec<String> = all.keys().cloned().collect();
        for id in ids {
            if all.len() < KEEP_ACCEPTED {
                break;
            }
            if all.get(&id).is_some_and(|r| r.state == State::Accepted) {
                all.shift_remove(&id);
            }
        }
        all.insert(key.to_owned(), Receipt { digest, expires: now + RETENTION_MS, state: State::Pending, result: None });
        if let Err(e) = Self::save(&all, self.key.as_deref()) {
            all.shift_remove(key);
            return fail(&e);
        }
        drop(all);
        let (receipts, store, id) = (self.receipts.clone(), self.key.clone(), key.to_owned());
        // `Promise.resolve().then(send)`: the send starts after this call has returned
        let result = spawn_task(async move {
            crate::platform::rt::yield_now().await;
            let r = send().await;
            let mut all = receipts.borrow_mut();
            match &r {
                Ok(_) => {
                    if let Some(rec) = all.get_mut(&id) {
                        rec.state = State::Accepted;
                        rec.expires = now_ms() + RETENTION_MS;
                    }
                }
                Err(_) => {
                    all.shift_remove(&id);
                }
            }
            let _ = Self::save(&all, store.as_deref());
            r
        });
        if let Some(rec) = self.receipts.borrow_mut().get_mut(key) {
            rec.result = Some(result.clone());
        }
        Box::pin(async move { result.wait().await.map(Some) })
    }
}

thread_local! {
    static SENDS: SendReceipts<Value> = SendReceipts::new(Some("send-receipts"));
}

/// The hub's own receipts, kept under "send-receipts".
pub fn send_once(key: &str, text: &str, send: impl FnOnce() -> LocalBoxFuture<'static, HubResult<Value>> + 'static) -> LocalBoxFuture<'static, HubResult<Option<Value>>> {
    SENDS.with(|s| s.send_once(key, text, send))
}

#[cfg(test)]
#[path = "send_receipts_tests.rs"]
mod tests;
