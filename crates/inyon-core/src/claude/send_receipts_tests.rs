//! claude/send-receipts.test.ts

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::{json, Value};

use super::*;
use crate::platform::rt::Deferred;
use crate::platform::store::{use_store, MemoryStore};
use crate::test_support::rt::run;

fn ok(v: Value) -> LocalBoxFuture<'static, HubResult<Value>> {
    Box::pin(async move { Ok(v) })
}

#[test]
fn in_flight_and_lost_response_retries_submit_only_once() {
    run(|| async {
        let r = SendReceipts::<Value>::new(None);
        let sends = Rc::new(Cell::new(0));
        let submit = |s: Rc<Cell<i32>>| {
            move || -> LocalBoxFuture<'static, HubResult<Value>> {
                s.set(s.get() + 1);
                ok(json!("accepted"))
            }
        };
        let key = crate::ids::random_uuid();
        let (a, b) = futures_util::future::join(r.send_once(&key, "hello", submit(sends.clone())), r.send_once(&key, "hello", submit(sends.clone()))).await;
        assert_eq!((a.unwrap(), b.unwrap()), (Some(json!("accepted")), Some(json!("accepted"))));
        assert_eq!(r.send_once(&key, "hello", submit(sends.clone())).await.unwrap(), Some(json!("accepted")));
        assert_eq!(sends.get(), 1);
        let e = r.send_once(&key, "different", submit(sends.clone())).await.unwrap_err();
        assert!(e.message.contains("different message"));
    });
}

#[test]
fn a_rejected_send_can_be_retried_with_the_same_id() {
    run(|| async {
        let r = SendReceipts::<Value>::new(None);
        let key = crate::ids::random_uuid();
        let e = r.send_once(&key, "hello", || Box::pin(async { Err(HubError::plain("disconnected")) })).await.unwrap_err();
        assert!(e.message.contains("disconnected"));
        assert_eq!(r.send_once(&key, "hello", || ok(json!("accepted"))).await.unwrap(), Some(json!("accepted")));
    });
}

#[test]
fn accepted_and_interrupted_sends_remain_protected_after_a_hub_restart() {
    run(|| async {
        use_store(Rc::new(MemoryStore::new()));
        let file = format!("send-test-{}", crate::ids::random_uuid());
        let before = SendReceipts::<Value>::new(Some(&file));
        before.send_once("accepted", "hello", || ok(Value::Null)).await.unwrap();
        let finish: Deferred<()> = Deferred::new();
        let f = finish.clone();
        let pending = before.send_once("interrupted", "world", move || {
            Box::pin(async move {
                f.wait().await;
                Ok(Value::Null)
            })
        });
        let pending = crate::platform::rt::spawn_task(pending);
        crate::platform::rt::yield_now().await;
        let after = SendReceipts::<Value>::new(Some(&file));
        let duplicates = Rc::new(RefCell::new(0));
        let d = duplicates.clone();
        after
            .send_once("accepted", "hello", move || {
                *d.borrow_mut() += 1;
                ok(Value::Null)
            })
            .await
            .unwrap();
        let d = duplicates.clone();
        let e = after
            .send_once("interrupted", "world", move || {
                *d.borrow_mut() += 1;
                ok(Value::Null)
            })
            .await
            .unwrap_err();
        assert!(e.message.contains("hub restarted"));
        assert_eq!(*duplicates.borrow(), 0);
        assert!(!crate::js::stringify(&kv::get_value(&file).unwrap()).contains("hello"));
        finish.resolve(());
        pending.wait().await.unwrap();
        kv::del(&file);
    });
}

#[test]
fn a_thousand_finished_sends_do_not_block_the_next_one_only_sends_still_going_count() {
    run(|| async {
        let r = SendReceipts::<Value>::new(None);
        for i in 0..1000 {
            r.send_once(&format!("done-{i}"), &format!("message {i}"), || ok(Value::Null)).await.unwrap();
        }
        assert_eq!(r.send_once("next", "one more", || ok(json!("sent"))).await.unwrap(), Some(json!("sent")));
        // a retry of a finished one is still answered from its receipt
        assert_eq!(r.send_once("done-999", "message 999", || Box::pin(async { Err(HubError::plain("sent twice")) })).await.unwrap(), Some(Value::Null));
    });
}
