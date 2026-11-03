//! The phone core's SSH plugin and secrets, handed to the platform: on Android, over JNI to
//! Kotlin's `SshMethods` (shared with the Capacitor app, so both keep the phone's key in the same
//! encrypted file). A call goes out with an id and its answer comes back from another thread
//! ([`reply`]); events arrive the same way ([`event`]) and are handed to the core on its own thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use inyon_core::platform::rt::spawn;
use inyon_phone::ssh::{OnEvent, PluginError, PluginResult, SshPlugin};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

type Answer = Result<Value, PluginError>;

static PENDING: Mutex<Option<HashMap<u64, oneshot::Sender<Answer>>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);
static EVENTS: Mutex<Option<mpsc::UnboundedSender<(String, Value)>>> = Mutex::new(None);

fn pending<R>(f: impl FnOnce(&mut HashMap<u64, oneshot::Sender<Answer>>) -> R) -> R {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    f(p.get_or_insert_with(HashMap::new))
}

/// What Kotlin's rejection carries: `{ message, code }`.
pub fn refusal(json: &str) -> PluginError {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    PluginError { code: v.get("code").and_then(Value::as_str).map(str::to_owned), message: v.get("message").and_then(Value::as_str).unwrap_or("failed").to_owned() }
}

/// An answer from Kotlin, for the call with this id.
pub fn reply(id: u64, ok: bool, json: &str) {
    let Some(tx) = pending(|p| p.remove(&id)) else { return };
    let _ = tx.send(if ok { Ok(serde_json::from_str(json).unwrap_or(Value::Null)) } else { Err(refusal(json)) });
}

/// An event from Kotlin: streamLines, streamExit, ptyData, ptyExit or closed.
pub fn event(name: &str, json: &str) {
    if let Some(tx) = EVENTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send((name.to_owned(), serde_json::from_str(json).unwrap_or(Value::Null)));
    }
}

/// Sends a call out: its id, the method and its JSON arguments. False when nothing can take it.
pub type Out = fn(u64, &'static str, &str) -> bool;

/// The plugin as the core sees it.
pub struct BridgeSsh(pub Out);

impl SshPlugin for BridgeSsh {
    fn call(&self, method: &'static str, args: Value) -> PluginResult {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        pending(|p| p.insert(id, tx));
        // Kotlin may answer before this returns (a missing argument), so nothing is held meanwhile
        if !(self.0)(id, method, &args.to_string()) {
            pending(|p| p.remove(&id));
            return Box::pin(async { Err(PluginError::plain("The phone's SSH isn't available. Close Inyon and open it again.")) });
        }
        Box::pin(async move { rx.await.unwrap_or_else(|_| Err(PluginError::plain("The phone's SSH stopped before it answered"))) })
    }

    fn listen(&self, on: OnEvent) {
        let (tx, mut rx) = mpsc::unbounded_channel::<(String, Value)>();
        *EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        spawn(async move {
            while let Some((name, v)) = rx.recv().await {
                on(&name, &v);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use inyon_core::platform::rt::install;
    use serde_json::json;

    use super::*;

    thread_local! {
        static SENT: RefCell<Vec<(u64, String, String)>> = const { RefCell::new(Vec::new()) };
    }

    fn record(id: u64, method: &'static str, args: &str) -> bool {
        SENT.with(|s| s.borrow_mut().push((id, method.to_owned(), args.to_owned())));
        // a missing argument is refused before the call returns
        if method == "probe" {
            reply(id, false, r#"{"message":"host is required","code":"BAD_REQUEST"}"#);
        }
        true
    }

    fn nowhere(_: u64, _: &'static str, _: &str) -> bool {
        false
    }

    struct Rt;
    impl inyon_core::platform::rt::Runtime for Rt {
        fn spawn(&self, fut: futures_util::future::LocalBoxFuture<'static, ()>) {
            tokio::task::spawn_local(fut);
        }
        fn sleep(&self, ms: u64) -> futures_util::future::LocalBoxFuture<'static, ()> {
            Box::pin(tokio::time::sleep(std::time::Duration::from_millis(ms)))
        }
        fn now_ms(&self) -> u64 {
            0
        }
    }

    // one test, since the answers and events go through process-wide tables
    #[test]
    fn calls_go_out_with_an_id_and_their_answers_and_events_come_back() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            install(Rc::new(Rt));
            let ssh = BridgeSsh(record);
            let call = ssh.call("getSecret", json!({ "key": "phone-key" }));
            let (id, method, args) = SENT.with(|s| s.borrow()[0].clone());
            assert_eq!((method.as_str(), args.as_str()), ("getSecret", r#"{"key":"phone-key"}"#));
            // the answer comes from another thread
            std::thread::spawn(move || reply(id, true, r#"{"value":"s3cret"}"#)).join().unwrap();
            assert_eq!(call.await, Ok(json!({ "value": "s3cret" })));
            // refused at once, while the call was still going out
            let refused = ssh.call("probe", json!({})).await.unwrap_err();
            assert_eq!(refused.code.as_deref(), Some("BAD_REQUEST"));
            // an answer for a call nobody waits for is dropped
            reply(9999, true, "{}");
            assert!(BridgeSsh(nowhere).call("exec", json!({})).await.unwrap_err().message.contains("open it again"));
            let heard = Rc::new(RefCell::new(Vec::new()));
            let h = heard.clone();
            ssh.listen(Rc::new(move |name, v| h.borrow_mut().push((name.to_owned(), v.clone()))));
            std::thread::spawn(|| event("closed", r#"{"id":"box","reason":"gone"}"#)).join().unwrap();
            for _ in 0..50 {
                if !heard.borrow().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(*heard.borrow(), vec![("closed".to_owned(), json!({ "id": "box", "reason": "gone" }))]);
        });
    }

    #[test]
    fn a_refusal_keeps_kotlins_code_and_message() {
        assert_eq!(
            refusal(r#"{"message":"Host key changed","code":"HOST_KEY_MISMATCH"}"#),
            PluginError { code: Some("HOST_KEY_MISMATCH".into()), message: "Host key changed".into() }
        );
        assert_eq!(refusal("garbage"), PluginError { code: None, message: "failed".into() });
    }
}
